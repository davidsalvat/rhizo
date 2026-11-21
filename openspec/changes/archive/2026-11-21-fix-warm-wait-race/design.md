# Design

## Context

Tests and benchmarks make templates the same way. They spawn a program
that warms up, prints a line (`warm`, `ready`, `start`) and pauses. They
step the host until the cell's output is exactly that line, then freeze
the cell (or suspend or fork it). The pause is where every copy starts
from. In CPython and MicroPython it is a 1 ms sleep, in Node.js a 50 ms
timer, in `copies` and `netstate` a 20 ms sleep, and in `libc_warm` a
`sched_yield`.

## Root cause

This is the race of change `fix-agent-turn-race` (its design, "What a step
lets the host see"). The host reads a cell's output only between slices.
When a call blocks, `run_slice` asks `threads.next(now_ns())` whether a
thread can run now. A sleeping thread whose deadline has passed can. A
sleep of 1 ms whose deadline is read before the vCPU is descheduled for a
millisecond is already over when it blocks, so the thread goes on in the
same slice. A `sched_yield` always ends the slice (`Outcome::Yield`), and
so does a sleep that cannot be over yet. Only a short sleep or timer, or
another thread that can run, lets the cell pass its pause unseen.

What the old wait does next depends on the program. Either way the
template is no longer frozen at its pause:

- **The rest of the output comes in the same slice.** The output is never
  exactly `warm\n` again, so the loop steps until the VM's time limit.
  This is the hang `fix-agent-turn-race` saw in `cpython_in_a_cell`.
- **The answer is long.** A later timer tick preempts the slice before the
  answer's line. The output is still `warm\n`, and the template is frozen
  mid-answer. Its copies print the right answer, but they start past the
  copy-generation check, so they do not reseed. A benchmark's warm figure
  would then measure part of an answer.

## Reproduction

An experiment, not committed, simulates a host stall in the `Block` arm
of `run_slice`. After a blocking call made while the cell's output ends in
`warm\n`, the slice asks `threads.next(now_ns() + 2_000_000)`, so the
pause after `warm` passes unseen. `cargo xtask --arch x86_64 test --only
ktest` (TCG, CPython built from the pinned tarball, Node.js packed) then
fails in `cpython::copies_draw_different_random` with `two copies drew
the same`. Both copies print `0.9429348595968395`: the template was frozen
mid-answer, after it seeded `random`. `cpython_in_a_cell` passes in that
run only because its answer was preempted before its line.

A second experiment stalls 60 ms after a block while the output ends in
`warm\n`, `ready\n` or `start\n`. That makes every short pause pass
unseen: CPython and MicroPython (1 ms), Node.js, the dynamically linked
`hello wait` and numpy (50 ms), and `copies` and `netstate` (20 ms). It
also showed the hang. Run while only the `warm` waits used this change,
`dynamic::a_dynamically_linked_cell_suspends_resumes_and_forks` timed out
waiting for exactly `ready\n`: `hello` had already printed the rest of its
output in the same slice. With every wait moved to this change, all
in-kernel tests pass under the 60 ms stall, and without it.

## Decisions

### D1: A blocking call ends the slice while the host waits for the pause

The host cannot see a pause that the slice skips, so the slice must not
skip it. `Live` gets a flag, `hold`. While it is set, a blocking call ends
the slice (`Back::File`) instead of asking `threads.next`. `file` then
puts the cell where it belongs: runnable if a thread can run (another
thread, or a sleep already over), otherwise asleep or waiting. The host
looks at the output after every such block. Only
[`Host::run_to_pause`] sets the flag, and it clears it once the cell has
paused. Forks and copies get a new `Live`, so they never inherit it.

This changes nothing for any other cell, and adds one test of a byte on a
path that already reads the clock and walks the threads. Steps run on the
boot CPU alone, so a gang never runs a held cell.

Rejected:

- **Lengthening the pauses.** It makes the race rarer, not impossible, and
  slows every run that pauses.
- **Making the programs wait for the host** (a file it writes, as the
  agent's `/branch`). They are also the Linux reference, and a host can
  stop them correctly without it.
- **Retrying or timing out.** A template frozen past its pause is wrong,
  however many times the test is retried.
- **Never letting a slice go on past a block.** It would change the
  scheduling of every cell, the benchmarked ones included.

### D2: Wait for the whole line, then the pause, in one helper

`Host::run_to_pause(id, line, pause)` reads the output with
`rhizo_core::line::Line`, a stream reader in the style of `Watch` from
`fix-agent-turn-race`. It finds the first complete line equal to `line`
however the bytes are split, and counts the bytes after it. The helper
steps until:

- the line is complete and nothing follows it, and
- the cell is at its pause. `Pause::Waits`: none of its threads can run
  (CPython, MicroPython, Node.js with every thread waiting, `copies`,
  `netstate`). `Pause::Slice`: the slice that printed the line ended
  (`libc_warm`, whose pause is a yield that leaves it runnable).

A cell that ends, or prints past its line, is an error naming what it
printed. Before this change, such a cell made the test spin until the VM
timed out, or froze a wrong template.

With `Pause::Waits`, a slice preempted between the line and the pause no
longer freezes the template there. The host steps once more, and the
held slice ends at the pause.

### D3: Where the reader lives

`Line` lives in `rhizo-core`, next to `rhizo_core::agent` and the other
text the kernel reads, so its property test runs on the host. It is a
module of its own: `agent::Watch` knows the agent's lines, and `Line`
looks for any one line.

## Risks

Benchmark figures. On an unloaded host the slice that prints `warm` ends
at its pause, so the templates freeze where they did:

- CPython and MicroPython asleep after `warm`;
- Node.js with every thread waiting, as `fast-node-template` D2 already
  required;
- `libc_warm` at its yield.

Only the template's own warm-up steps differently: a blocking call ends
its slice where another thread could have gone on. No benchmark measures
that warm-up. They measure cold runs and forks, which never hold.

On x86_64, `cargo xtask bench --icount-only` gives the same count for all
70 metrics it measured, before and after this change, with CPython and
Node.js. That includes `workload.cpython_warm_insns` and
`workload.warm_insns`. A template frozen at another point would change its
copies' counts. With MicroPython built too, every metric stays within the
gate. On AArch64, all 53 metrics give the same count before and after.
