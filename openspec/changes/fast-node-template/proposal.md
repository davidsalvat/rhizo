# Proposal

## Why

On CI with KVM (x86_64), a Node.js cell forked from its warm template
(`workload.node_warm_us`, 57–58 ms) takes longer than a cold start from
its files (`workload.node_cold_us`, 47–48 ms), although the fork skips
`ld.so`, Node.js and V8 start-up and the warm-up. CPython (4.7 ms warm)
and numpy (12 ms warm against 63 ms cold) show what a fork should give.

The difference is exactly the script's 50 ms pause after `warm`: every
fork waits it again. The template is frozen while its main thread waits
in `epoll_pwait` with the timer's timeout (50 ms). A call waiting on
descriptors is made again when its thread wakes, and the deadline it
keeps lives in the host's record of the live cell, not in the cell: a
template keeps none. So a fork's call, made again, started a new 50 ms
timeout. Spawning from a template "expires every deadline", but that
expiry only reached sleeps and futex waits. A wake from the host
(`Host::wake`) had the same flaw from the other side: the call made
again found its old deadline and waited again.

Under QEMU `-icount`, where a slice ends at other moments, the template
happened to be frozen before its main thread reached `epoll_pwait`, so
the instruction counts showed none of it (28 M instructions warm against
250 M cold): the benchmark froze the template wherever the slice that
printed `warm` ended.

## What Changes

- **Waits on descriptors keep no time either.** Spawning from a template
  and a wake from the host time out the threads' waits on descriptors
  (`poll`, `select`, `epoll_wait` with a timeout) as they end sleeps and
  timed futex waits: the call, made again, finds its deadline passed and
  returns what is ready, or nothing.
- **The benchmark freezes the template in its pause.** The Node.js
  template is frozen once every thread waits after `warm`, not wherever a
  slice ended, so every VMM, `-icount` included, measures forks from the
  same state.
- **Tests**: an in-kernel test with a cell waiting an hour in
  `epoll_pwait` (the `sleeper` cell's new `epoll` mode): a fork of it
  frozen there, and the same cell woken by the host, both time out at
  once.

### Out of scope

Snapshots: a suspended thread waiting on descriptors is recorded as about
to make its call again, without the time left, so a resumed cell starts
that call's timeout over. Recording it changes the snapshot format,
which open change `add-shared-snapshots` also changes.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `fork`: templates keep no time on waits on descriptors either.
- `benchmarks`: the Node.js template is frozen in its pause.

## Impact

`kernel/src/cell.rs` (`Live::expire_timers`, `Host::waits`, the Node.js
benchmark), `kernel/src/tests.rs`, `cells/src/bin/sleeper.rs` and two
system-call numbers in `cells/src/lib.rs`. `workload.node_warm_us` drops
by the 50 ms pause (from 79.5 M to 29.6 M instructions under `-icount`
with the template frozen in its pause); no other benchmarked path waits
on descriptors in a template or when woken.
