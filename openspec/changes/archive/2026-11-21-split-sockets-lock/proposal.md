# Proposal

## Why

Every system call of every cell takes the kernel's sockets lock (`NET`,
design D9 of `add-smp-scheduling`), whether or not it uses a socket. A
`getpid`, a pipe read or a file write on one CPU therefore waits for the
calls of the cells on the other CPUs. With several CPUs, the system calls
of all cells take turns. (The scheduler's own use of the lock, to drain
woken waiters, is already gated by a flag since `add-socket-wait-queues`.)

## What Changes

- **Calls take the sockets lock only when they use a socket.** The ABI
  reaches the sockets through a `Sockets` accessor. The kernel locks them
  the first time a call uses them, and keeps the lock until the call
  ends, so a call still sees the sockets and joins their wait queues in
  one critical section (no lost wake-up). Pipes, event counters, `epoll`
  instances without sockets, files, memory, time and threads never touch
  the lock. `poll`, `select` and `epoll_wait` lock it only when a
  descriptor they look at is a socket.
- **`SpinLock::lock`** returns a guard, for a holder that turns out to
  need the lock in the middle of its work.
- **Metric**: `cell.syscall_scaling_pct`. Four `piper` cells per CPU, each
  writing and reading a byte through its pipe 20,000 times, are run on
  every CPU and on one CPU after the other. The metric is the parallel time
  times the number of CPUs over the serial time: 100 % is perfect scaling,
  400 % on 4 CPUs is fully serialized.

### Out of scope

Calls of different cells on sockets still take turns on the one lock: the
loopback queues of a connection, the listeners that `connect` finds and
the ports that `bind` checks are shared between cells. Per-socket locks,
with the smoltcp interface behind its own lock, are left for a change
that measures socket calls on several CPUs. Today the network card and
socket benchmarks run on one CPU, and the server workload stays on one
vCPU because host operations stop every CPU.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `multi-core`: system calls that use no socket never wait for the
  sockets lock.
- `benchmarks`: the system-call scaling metric.

## Impact

`crates/rhizo-core/src/sync.rs` (`Guard`), `kernel/src/sync.rs`
(`Global::lock`), `crates/rhizo-abi` (`Sockets`, `Context::net`),
`kernel/src/cell.rs` (`CallNet`, `WOKEN`, `multi_core`), `xtask/src/bench.rs`,
`bench/README.md`, `bench/baseline.json`.
