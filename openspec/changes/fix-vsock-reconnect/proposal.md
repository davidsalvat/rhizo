# Proposal

## Why

The `control-plane` spec says the control kernel says hello again on
each connection the host makes to vsock port 1024, after which its frames
take that connection. Reading the vsock code as an auditor for the
security documentation of `control-over-vsock` showed that a new
connection is missed when it replaces the old one between two of the
control loop's looks:

- `kernel/src/control.rs` tells connections apart only by
  `vsock::connected()` changing (`Server::on_vsock`, a `bool`), and
  `kernel/src/vsock.rs` drops the `Change` that `Stream::take` returns.
- The control loop reads the device only between cell slices. When the
  host resets its connection and connects again before the next poll (a
  restarted server, or a client that gave up and retried), both packets
  are taken in one poll: the stream is connected before and after, so
  the kernel sends no hello on the new connection, and the decoder keeps
  the old connection's state: the start of a frame the old connection
  never finished is completed with the new connection's bytes, which the
  decoder can then accept as a frame nobody sent (frames carry no
  checksum).

No tool reconnects today (`rhizo up` holds one connection for the VM's
life), so this does not fail in CI; it breaks the spec's second hello and
lets one connection's bytes be read as another's. The full analysis is
in `design.md`.

## What Changes

- **Connections have numbers.** `rhizo_virtio::vsock::Stream::connection`
  gives the current connection's number, a new one for every connection
  made, and `Stream::read` takes the number of the connection to read
  from and gives nothing once another connection took its place.
- **The control kernel follows numbers, not a flag.** `Server` keeps the
  number of the connection its frames take; any change (a new connection,
  even one that replaced another between two looks, or none) starts a
  fresh decoder and greets a new connection. Bytes of a new connection
  are not read until then.
- **Tests**: a host test drives the driver and the stream through the
  simulated device with a reset, a new request and the new connection's
  bytes in one poll, after half a frame was left unread.

### Out of scope

Who may connect (on QEMU any local user can reach a guest's vsock port;
see O8 in `docs/security.md`), and requests of an ended connection still
answered on the next one.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `control-plane`: a connection that replaces another between two looks
  of the control kernel is greeted and starts its frames afresh.

## Impact

`crates/rhizo-virtio/src/vsock.rs` (and its tests), `kernel/src/vsock.rs`,
`kernel/src/control.rs`: control builds only. One comparison per byte read
from the control channel; no benchmarked path changes.
