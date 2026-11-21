# Design

## D1. Count first, then the periodic mode

A local APIC timer restarts when its initial count is written, in the mode
the LVT gives at that moment. Restarting the periodic tick after a timed
wait takes two writes, and only their order is ours to choose:

| Order | State between the writes | Effect |
|---|---|---|
| Mode, then count (before) | periodic, with the wait's count (1 to 625,000) | a VMM that emulates the timer may fire it every 16 ns, holding its lock |
| **Count, then mode (chosen)** | one-shot (or masked), with the tick's count | at most one early expiry, which `take` treats like any tick |
| Count 0, mode, count | stopped | a third write: one more VM exit per wait |

Every caller of `tick()` leaves the timer one-shot or masked first
(`init`, `start_tick`, `tsc_hz`, `wait_ns`), except `set_tick_ns`, which
changes the period of a timer that is already periodic. In that case the
order makes no difference.

How it was found: the test bounded at 8 s never reported, so the boot CPU
was stuck. QEMU's monitor did not answer, because it needs the same lock.
A host backtrace of the QEMU process showed four vCPU threads waiting for
`qemu_mutex_lock_iothread` and the main thread in
`qemu_clock_run_all_timers`, at 90% of a host CPU. With the writes swapped,
360 runs of the test in a row (40 per boot) never stalled; before, it
stalled within about five.

The regression test replays `wait_writes(ns)` and then `tick_writes(TICK)`
on a model of the two registers. A test against the APIC itself cannot see
a state that lasts two instructions, and how long it lasts depends on the
VMM.
