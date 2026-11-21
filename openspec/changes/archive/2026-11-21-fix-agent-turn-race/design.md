# Design

## Context

`agent()` (`kernel/src/cell/agent.rs`) runs the CPython agent
(`cells/python/agent_loop.py`, `cell` mode) for N turns on the boot CPU,
one `Host::step` at a time, and suspends and resumes it after every turn.
Each turn the agent sends a request over its kept-alive connection, reads
the answer, runs the tool, prints `turn N …`, then sleeps `PAUSE` (1 ms).
After the last turn it prints `fork` (in `timed` mode, two `agent …_ns=`
lines first) and polls for `/branch`, sleeping 10 ms each time.

Before this change the host waited, after every step, for

    printed (a new complete line) && (asleep || last line starts "turn ")

then broke out to the fork if the output ended in `fork\n`, and otherwise
suspended. A turn was therefore recognized only if the host looked while
the turn's line was the last line.

## Root cause

### What a step lets the host see

`Host::step` runs one slice of the cell (`run_slice`, `kernel/src/cell/slice.rs`)
until it ends, yields, has no thread that can run, or is preempted (second
timer tick). When a system call blocks, the slice asks
`threads.next(now_ns())` whether a thread can run *now*, and a sleeping
thread whose deadline has passed is runnable again (`Thread::expire`): it
continues in the same slice without the host looking.

CPython's `time.sleep(0.001)` reads `CLOCK_MONOTONIC` and calls
`clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, now + 1 ms)`. If 1 ms of
guest time passes between that clock read and the kernel's
`next(now_ns())` after blocking, the pause is over before the slice ends.
Under TCG the guest clock follows the host's, so a vCPU thread
descheduled for a millisecond by the host is enough. The failing job ran
the in-kernel tests while the host tests and the Node.js step ran on the
same runner (`in-kernel tests ... FAILED (64.4 s)`, then `host tests ...
ok (80.0 s)`, `Node.js in cells on 2 CPUs ... ok`).

### Which boundaries can be missed

The network card is polled only by the host between slices (`poll_wire` in
`Host::step` and in its waits; no system call polls it). Between the lines
of turns N and N+1 the agent must read the model's answer, which cannot
arrive inside the slice that sent the request: the agent blocks in `recv`,
the slice ends, and the host sees turn N's line as the last line. So turns
1 … N-1 are always seen.

After the **last** turn nothing waits on the network: line, 1 ms pause,
`fork`. If the pause passes unseen as above, the first piece the host sees
ends `turn N …\nfork\n`. The old loop matched `fork\n` and went to the
decision point: one suspend short, exactly `left: 2, right: 3`, with the
transcript still correct (the run returned `Ok`, so the assertion on the
count, which comes first, failed). No other path gives a count of N-1:
an earlier turn cannot be missed, and nothing suspends twice.

### Leads ruled out

- **Two turns' outputs in one read**: impossible for the reason above
  (the answer of turn N+1 needs a host poll of the card, which needs the
  slice to end).
- **A turn ending while the cell is blocked on the network**: the turn's
  line is printed after its answer is read; a cell blocked in `recv` ends
  its slice, and the host then sees the line last.
- **`WIRE_PERIOD_NS` and the card's poll interval**: they only delay when
  the answer arrives; the host still looks between the slice that blocks
  in `recv` and the one that prints the line.
- **A connection of an earlier test still closing on the card** (the lead
  of `shard-kernel-tests`, D1–D2): it shares the card and can delay
  frames, i.e. stretch the waits on the network, which are the boundaries
  that are always seen. It cannot make the agent's last line and `fork`
  arrive in one piece; only the time between the clock read and the
  block does.
- **Control over vsock, or the order of tests in shards**: vsock is compiled only into the control kernel (feature `control`), not
  into the in-kernel test kernel, and neither is on the path between
  the agent's last line and `fork`; at most they change load, which is
  the trigger, not the cause.

### Reproduction

An experiment, not committed, simulates a 2 ms host stall in `run_slice`:
after a blocking call made right after the agent printed a turn's line
(its pause), the slice asks `threads.next(now_ns() + 2_000_000)`, so every
pause passes unseen. On x86_64 (QEMU, TCG, CPython built from the pinned
tarball), `cargo xtask --arch x86_64 test --only ktest` then fails with
exactly `suspended after every turn`, left 2, right 3, although the pauses
after turns 1 and 2 pass unseen too (their lines are still read last, as
argued above). With this change and the same stall the test passes. The
host test `a_turn_and_the_fork_in_one_piece_are_both_seen` feeds the exact
piece of that failure.

The same stall applied to every blocking call also hangs
`cpython_in_a_cell` after `warm`: hosts that wait for a template's output
to be exactly `warm\n` before freezing it have the same race (out of
scope here; proposed as a separate change).

## Decisions

### D1: Read every line, not the last one

`rhizo_core::agent::Watch` reads the output a piece at a time and returns
the complete lines it found: how many start `turn `, and whether one ends
in `fork`. It keeps only the current line's first 5 and last 4 bytes, so
a line split anywhere is found once. `agent()` feeds it the bytes of the
running cell's output it has not read yet (from 0 again after a resume,
whose output starts empty), and:

- suspends and resumes after a piece holding a turn's line, whether or
  not the decision point came in the same piece (a cell is suspended at
  any point between slices; the old loop already suspended without
  waiting for the pause);
- freezes the agent once the decision point has been read and the agent
  sleeps (its safe point, design D3 of `add-agent-workload`), as before.

Rejected: making the agent wait for the host between turns (a file the
host writes, as for `/branch`). It changes the program that is also the
Linux reference, adds a wait to each turn's measured time, and the host
can read its output correctly without it. Also rejected: lengthening
`PAUSE`, which makes the race rarer, not impossible.

### D2: More than one turn in a piece is an error

By the argument above it cannot happen; if a future change made it
possible, the run returns an error naming the turns that ended before the
host looked, rather than suspending once for two turns and failing on a
count.

### D3: Where the reader lives

In `rhizo-core`, so its property test runs on the host: the kernel crate
cannot run host tests, and `rhizo-core` already holds other text the
kernel reads (boot options, reports).

## Risks

None for the benchmarks: the agent figures (`agent_figures`) take the
same suspends and forks; the reader is a byte loop over output the host
already copies on every step.
