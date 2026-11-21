# Proposal

## Why

Change `fix-agent-turn-race` found that the host reads a cell's output
only between the cell's slices (`Host::step`), and that a slice goes on
past a blocking call when a thread can run at once
(`threads.next(now_ns())` in `run_slice`). A sleep whose deadline passed
before it blocked is already over, and the thread continues in the same
slice without the host looking. The deadline can pass that way when 1 ms
of CPython's `time.sleep(0.001)` goes by while the host deschedules a TCG
vCPU.

Every in-kernel test and benchmark that freezes a template after its
`warm` line has the same race. It steps until the output is exactly
`warm\n`, then freezes. If the pause after `warm` passes unseen, one of
two things happens. The output goes past `warm\n` and the loop spins
until the VM times out. Or the slice is preempted later in the answer,
and the template is frozen mid-answer, past the point every copy must
start from. Templates frozen after a 50 ms pause (Node.js, numpy, the
dynamically linked `hello`) and the tests that freeze `copies` and
`netstate` after a 20 ms pause have the same race, less often.

The reproduction is in `design.md`. With a simulated host stall,
`copies_draw_different_random` (CPython) fails with `two copies drew the
same`, because the template was frozen after it had already checked its
copy generation.

## What Changes

- **`Host::run_to_pause(id, line, pause)`**, one helper for every such
  wait. It steps until the cell has printed the whole line and nothing
  after it, and has paused. `Pause::Waits` means none of its threads can
  run. `Pause::Slice` means the slice that printed the line ended, for
  programs that `sched_yield`. Meanwhile a blocking call of that cell ends
  its slice, so a pause that is already over still gives the host its
  look. It reports a cell that ended or printed past the line.
- **`rhizo_core::line::Line`**, the reader it uses. It finds the first
  complete line equal to the one wanted however the output is split, and
  counts the bytes after it. It has host tests and a property test with
  arbitrary splits.
- **Callers**: the template tests and benchmarks of CPython, MicroPython,
  Node.js, numpy, the dynamically linked `hello` and `libc_warm`, and the
  tests that freeze or suspend `copies` and `netstate` after their 20 ms
  pause.

### Out of scope

- Programs whose pause is a `sched_yield` alone in a test that does not
  freeze a template (a yield always ends the slice), or a sleep of a
  second or more.
- Waits for lines that end a program.
- Changing the programs: they are also the Linux reference.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workloads`: templates are frozen at the pause after their line however
  the host reads their output.

## Impact

- `crates/rhizo-core/src/line.rs` (new, host-tested).
- `kernel/src/cell/pause.rs` (new), and one flag on `Live`, checked where
  a blocking call may continue its slice (`kernel/src/cell/slice.rs`).
- The callers in `kernel/src/tests/` and `kernel/src/cell/bench/workloads.rs`.

Benchmark templates freeze where they did on an unloaded host: asleep
after `warm`, every Node.js thread waiting, `libc_warm` at its yield. So
their figures do not move. Cells that are not being run to a pause never
see the flag.
