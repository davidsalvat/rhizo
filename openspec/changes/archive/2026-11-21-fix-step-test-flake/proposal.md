# Proposal

## Why

The in-kernel test `suspend_resume_after_every_step_is_invisible_eager`
failed once on x86_64 CI: the counter cell was suspended 39 times, and the
test asks for 40 at least, one per step of its work. The lazy variant has
also run for 30 s without output on a loaded host.

Both come from the test's setup, not from a kernel bug:

1. `counter` ended each step of its work with `sched_yield`, except four,
   which it ended with a 1 ms sleep instead. A sleep that is already over
   when its thread blocks (the VM stalled for 1 ms between the call and the
   check) leaves a runnable thread, and the kernel runs it on in the same
   slice (spec `scheduling`, "Threads share their cell's time"). That step
   and the next then end in one scheduling step, so the host suspends one
   time fewer. The kernel is right; the count depended on timing. With a
   2 ms stall injected at block time, the test fails on every run (37
   suspensions).
2. Each round of the test spent about 160 ms of an unoptimized kernel in
   `Snapshot::capture`, zeroing the snapshot's 4 MiB of page data a byte at
   a time (`Vec::resize`): about 7 s of each variant's 8 under QEMU, and
   several times that on a loaded host. Nothing hangs; the test prints only
   when it ends.

## What Changes

- `counter` ends every step with `sched_yield`, after its sleep where it
  sleeps, as its own description already says: every step ends a scheduling
  step whatever the clock does, and a sleep still in progress adds one. The
  test, its assertion (40 suspensions at least) and the program's output
  are unchanged.
- `Snapshot::capture` allocates its buffer zeroed at its final size
  (`vec![0; n]`, zeroed with `memset`) and fills it in place, instead of
  growing it with `Vec::resize`. The snapshot's bytes are unchanged.
- The `snapshots` spec's equivalence scenario says what a step is, so that
  the test checks it deterministically.

### Out of scope

Eager resume, and the suspension of a lazily resumed cell, take most of
what is left of the test's time (about 1 s per variant); they copy pages
and run no zeroing loop.

## Capabilities

### Modified Capabilities

- `snapshots`: the equivalence scenario names the steps the cell is
  suspended at.

## Impact

- `cells/src/bin/counter.rs`, `crates/rhizo-snapshot/src/lib.rs`.
- Each variant of the test takes 1.2 to 1.4 s instead of 6 to 8 s on both
  architectures (unoptimized kernel under QEMU).
- Release builds: `cell.suspend_insns` goes from 2,315,390 to 2,315,751
  instructions (+0.016 %, x86_64 under `-icount`), within the 2 % gate;
  no other instruction count moves.
