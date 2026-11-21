# Design

## Context

`Host::run_to_pause(id, line, pause)` (change `fix-warm-wait-race`) steps
the host until cell `id` has printed `line` and nothing after it, and has
paused. With `Pause::Waits` that means none of its threads can run. While
it runs, the cell is held (`Live::hold`): a blocking call ends its slice,
so the host looks after every block. Each `Host::step` runs one slice of
the cell that has waited longest.

## Root cause

The failing runs' console (jobs 137021217429 and 137021562080) shows
cell 7, the template, printing `warm`. Then three answers follow: the
resumed cell's, its two forks', and a fourth one, the template's own,
61 ms after its `warm`. Cell identifiers are global. `node_answers_as_on_linux`
takes cells 1 and 2, and this test's warm cell, resumed cell and forks
take 3 to 6, so 7 is the template. `run_to_pause` returned `cell 7
printed past its pause`. The test passed that error to `ended(host, 7)`,
which panicked with `cell 7 has not ended` and hid it.

Node.js pauses with `setTimeout(50)`. Its main thread blocks in
`epoll_pwait` with a 50 ms timeout. As it does, a V8 platform worker it
woke during the warm-up can still run. The held slice ends at the block,
and `file` requeues the cell as runnable. Round robin then runs the three
other runnable cells, each for a full 10 ms slice and longer in a debug
kernel paging in a lazy resume. Then the worker runs and blocks, and the
slice ends again. After a round or two the 50 ms are over, and the main
thread's next slice prints the answer. The host never saw every thread
wait.

A single-threaded template cannot hit this. Its block files it asleep or
waiting, and the host sees that right after the slice. The window needs
both another runnable thread in the template and other runnable cells in
the host. In `ktest-node`, the second warm-up of
`a_node_cell_suspends_resumes_and_forks` runs next to the resumed cell
and its forks.

## Reproduction

- **In-kernel, committed.** `a_threaded_pause_is_seen_next_to_busy_cells`:
  `threads pause` prints `ready`, wakes a thread that yields four times
  and then waits, and sleeps 20 ms. Three `spin` cells are runnable
  beside it. Without the fix, `cargo xtask --arch x86_64 test --only
  ktest` fails every time: `cell … ended before it paused (Exit(0)),
  having printed "ready\ndone\n"`.
- **Node.js, not committed.** Three `spin` cells spawned before the
  template of `copies_draw_different_random`. Without the fix,
  `RHIZO_NODE=1 cargo xtask --arch x86_64 test --only ktest-node` fails
  with the CI error, now shown in full: `cell 13 printed past its pause:
  "warm\n{…answer…}\n0.59… e64df074-…\n"`. With the fix it passes.
- **Host model, committed.** `a_pause_behind_busy_cells_passes_unseen_in_turn`
  in `rhizo-sched`: round robin over three busy cells lets a 50 ms pause
  pass unseen, and taking the held cell first does not.

## Decisions

### D1: Once its line is out, the held cell runs before any other

While the cell has printed its line and nothing after it, `run_to_pause`
runs its next slice whenever it can run (`step_held`), before any other
cell. It takes the cell with `Scheduler::take` (runnable entries only) and
does the bookkeeping `take` in `sched.rs` does: nudge, readiness, running
count, the CPU's cell. It runs the slice on the boot CPU, as `step`
does. If the cell cannot run, a plain `step` runs another cell or waits.

The cell then reaches its pause in its own CPU time, as on an unloaded
host. Before the line, steps stay round robin: a cell may need other
cells to reach its line (a server waiting for a client).

The host is the right place for this. The pause is the program's, and a
host that waits for one cell to pause owes it the CPU until it does. With
nothing else runnable, which is every benchmark, `step_held` runs the
very slice `step` would.

Rejected:

- **A longer timer in `demo.js`.** It makes the race rarer, not
  impossible, and it changes the Linux reference.
- **Freezing when the main thread alone waits.** The kernel does not know
  which thread is the program's main loop, and `fast-node-template` D2
  freezes with every thread waiting so that every VMM freezes the same
  state.
- **Stopping the cell's clock while it is held.** Time would differ
  between the template and Linux, and a held cell would see time jump.

### D2: Errors carry their evidence

`run_to_pause` names what the cell printed in both errors. For a cell
that ended, it also gives the ending, read from `Host::finished`. Callers
panic with the error alone. They never call `ended`, which panics while
the cell is live.

## Other templates

- **numpy** (`ktest-packages`) pauses with `Pause::Slice`, so a held slice
  that ends is its pause: there is no timed window. The numpy benchmark's
  template warms alone.
- **The dynamically linked `hello wait`, `copies`, `netstate`, CPython,
  MicroPython**: single-threaded. Their block files them asleep, so they
  have no window. `copies` and `hello` are still frozen next to runnable
  forks, and D1 covers them as well.
- **`libc_threads`**: never run to a pause. Its tests step until its
  output is `ready\n` while its workers wait on a condition variable with
  no deadline, so no time window exists.

## Risks

Benchmarks: every template is warmed with no other runnable cell, so its
steps are the same slices. `cargo xtask bench --icount-only` checks this.
