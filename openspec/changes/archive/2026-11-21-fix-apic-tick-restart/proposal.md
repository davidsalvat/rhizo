# Proposal

## Why

`ktest-smp` on x86_64 stalls about one run in eight (QEMU TCG, 4 vCPUs).
It shows most often in `a_woken_cell_runs_at_once`, whose last console line
is `ready`, and it looked like the wake the host sends after `ready` was
lost (as in design D5 of `add-socket-wait-queues`). It is not a lost wake,
and the stall is not specific to that test: it also stops
`a_cell_on_several_cpus_survives_suspend_and_fork`. When the run stalls:

- the test's own 8 s bound (`host.run` stop condition) never fires: the
  boot CPU makes no progress either;
- every vCPU thread of QEMU waits for QEMU's global lock
  (`qemu_mutex_lock_iothread`), and the main thread uses a whole host CPU
  inside `qemu_clock_run_all_timers`, firing a timer again and again.

The timer is the local APIC timer of a CPU coming back from
`irq::wait_ns`. `tick()` restarted the periodic tick by writing the
periodic mode first and the count second. In between, the timer is
periodic with the count `wait_ns` left in it, which is a single count
(16 ns) for a wait of under 32 ns, as for a deadline that is already due.
QEMU then re-arms the timer every 16 ns, faster than it can run the
callbacks, while holding the lock the vCPU needs for its second write. The
VM never gets the lock back. KVM's in-kernel APIC limits periodic timers to
a minimum period, so under KVM the same window costs at most a few extra
interrupts. Firecracker's CI timeout on `ad8ad2c` came before
`add-socket-wait-queues` and is the race that change fixed.

## What Changes

- **The tick restarts count first**: `tick()` writes the initial count
  while the timer is still one-shot (or masked), then switches it to
  periodic. It makes the same two writes, in the other order. The writes
  of the restart and of the wait are named sequences (`tick_writes`,
  `wait_writes`), so an in-kernel test can replay them.
- **A deterministic regression test**: `the_timer_is_never_periodic_with_a_short_count`
  replays a wait of any length and the restart on a model of the timer's
  mode and count, and fails after any write that leaves it periodic with
  less than the tick. It fails with the old order.

### Out of scope

AArch64: its generic timer has no periodic mode (every expiry re-arms a
one-shot), so it has no such window. `a_woken_cell_runs_at_once` and
`a_wake_before_the_sleep_is_kept` stay as they are.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `multi-core`: a CPU leaving a timed wait goes back to its tick without
  ever making its timer periodic with a shorter period.

## Impact

`kernel/src/arch/x86_64/irq.rs`, `kernel/src/arch/x86_64/tests.rs`. No
benchmarked path changes cost: the same two register writes per wait.
