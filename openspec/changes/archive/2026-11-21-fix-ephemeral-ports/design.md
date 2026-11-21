# Design

## D1. The boot picks where ephemeral ports start

`Net::set_boot` already receives the kernel's randomness for this boot,
which identifies parked connections. It now also sets the next ephemeral
port. It mixes the value (the SplitMix64 finalizer, because boot
identities taken from a cycle counter are close to each other) and takes
it modulo the size of the range (32768 to 60999). Allocation then goes on
in order from there and skips the ports in use, as before. The behavior
inside one boot does not change, and a `Net` that was given no boot
identity (the host tests) starts at 32768, as before.

Two boots reuse a port only when their starting points are within the
number of connections a boot opened; with the 28,232 ports of the range,
the 20 to 25 connections of a laptop run collide with the previous run's
with a probability under 0.2 %.

Lost:

- A random port per connection (RFC 6056 algorithm 1). It changes the
  order within a boot, which the host tests and the parked connections of
  `keep-tcp-across-suspend` do not need, and it costs a draw per
  connection.
- Initial sequence numbers above those of the previous boot (RFC 6528).
  smoltcp draws them, and the kernel's clock starts again at every boot.
- Waiting for `TIME_WAIT` to end: up to 60 s on Linux.

## D2. Evidence and gate

The draft #71 measured the phases of the slowest turn
(connect, request sent, first answer bytes, end) and the card's polls and
frames. That diagnostic is not kept. The CI benchmark of this change must
show `workload.agent_turn_us` and `laptop.resume_turn_us` on Firecracker
and Cloud Hypervisor in the same range as on QEMU, in every run. They then
leave `ungated.ci`, with the values of two agreeing CI runs in the
baseline.
