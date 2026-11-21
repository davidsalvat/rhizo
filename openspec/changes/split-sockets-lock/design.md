# Design

## D1. The sockets are locked by the first use in a call

`rhizo_abi::Context::net` becomes `&mut dyn Sockets`. `Sockets::net()`
returns the `Net`. The host tests pass the `Net` itself, which implements
it. The kernel passes a `CallNet`, which takes `NET` the first time
`net()` is called (a `SpinLock` guard) and drops it when the system call
ends, after the call has joined the wait queues of the sockets it looked at
(`Net::end_call`). A call that uses sockets therefore holds the lock from
its first look to its registration, as before: the wake-up argument of
`add-socket-wait-queues` (D2) is unchanged. A call that uses none takes
no lock.

`poll`, `select` and `epoll_wait` ask the file tables for the events of each
descriptor, which ask the sockets only for socket descriptors. The ABI
gives them a closure that locks on its first call, so a `poll` on pipes and
event counters stays lock-free.

The lock order is unchanged: the cell's lock (when it runs on several
CPUs), `SHARING`, then the sockets.

Lost: passing the lock as a type parameter of `Context`, which every
function of the ABI names; and locking per call kind in the kernel's
dispatch, which would duplicate the ABI's knowledge of which calls use
sockets.

## D2. Waiters are drained only when a change woke some

This part landed with `add-socket-wait-queues` (its D2: the `WOKEN` flag),
which needed it to keep a switch between cells as cheap as before. A call
that locked the sockets sets the flag, as it ends, if its changes woke
waiters; a call that never locked them changed nothing.

## D3. Measuring it

`cell.syscall_scaling_pct` uses the 4-vCPU `bench-cell` boot, next to
`cell.scaling_pct` (the same with `spin`, which makes no system calls). Four
`piper` cells per CPU each write and read a byte through their pipe
20,000 times. The run is timed on every CPU (`Host::run`) and on one CPU
after the other (`Host::step`); the median of 3 rounds is kept, and the
metric is the parallel time times the number of CPUs over the serial time.
Before this change, every call took `NET`. Comparing the metric with the
lock taken eagerly (the old behavior) and lazily shows what the split
gives: see the pull request.

## D4. What is left

Socket calls of different cells still take turns on `NET`. Splitting it
per socket means locking a connection's two ends (bytes are queued on
the peer), the listener (`connect`), the port table (`bind`, `connect`)
and the smoltcp interface separately, in an order that cannot deadlock,
and it touches every operation of `rhizo-net`. No benchmark yet makes
socket calls on several CPUs: the network card and socket benchmarks use
one CPU, and the server workload stays on one vCPU. The next step,
measured first, is a multi-CPU loopback benchmark.
