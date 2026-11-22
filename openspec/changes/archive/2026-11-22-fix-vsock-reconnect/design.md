# Design

## Context

The control kernel (`kernel/src/control.rs`, feature `control`) takes
its frames from the serial line, or from the host's vsock connection
while there is one (design D2 of `control-over-vsock`). The device is
polled: `vsock::poll()` moves the packets the device received into the
`Stream` (`rhizo_virtio::vsock`), which holds one connection at a time.

## Root cause

`Server::read` decided which channel to use from `link::on_vsock()`, a
`bool` (`Stream::connected`), compared with the last value it saw. The
`Change` values `Stream::take` returns (`Connected`, `Closed`) were
dropped by `pump`. A reset of the old connection and the request of a new
one taken in the same `pump` leave `connected()` true before and after,
so:

1. no hello is sent on the new connection (the spec's second hello;
   `xtask`'s `Vm::hello` waits 10 s for it, then fails);
2. `self.decoder` is not renewed: if the old connection ended inside a
   frame, its header and part of its payload are completed with the new
   connection's first bytes. The decoder checks the version, kind and
   length, but there is no checksum, so the splice can decode as a
   request;
3. in the closure that `Host::run` polls between slices, `link::read_byte`
   took the new connection's bytes into the same decoder before
   `Server::read` could see anything (even a change seen across two polls
   reached the decoder that way).

`pump` runs whenever the control loop looks for input: between cell
slices and every `LOOK_FAST_NS` or `LOOK_SLOW_NS`. A host that resets and
connects again within that time (a few milliseconds while a cell runs) has
both packets taken together.

## D1. A number per connection, in the stream

`Stream` counts the connections made (`made`, wrapping) and
`connection()` returns that count while a peer holds the connection.
`read(connection)` gives a byte only while `connection` is still the
current one. The guarantee that a new connection's bytes never reach an
old reader lives in the library, where the host tests reach it; the
kernel only carries the number.

Alternatives that lost: acting on the `Change` values in `pump` (the
kernel would keep a latch that the stream already knows how to keep, and
the bytes would still need gating); clearing the receive buffer on a
reconnect (the new connection's bytes are its host's requests and must be
read, only not by the old decoder).

## D2. The control kernel follows the number

- `Server::vsock: Option<u32>` replaces `on_vsock: bool`: the connection
  its frames take, or `None` for the serial line.
- `link::read_byte(on)` reads from vsock connection `on`, or from the
  serial line when `on` is `None` and no connection is held; when the
  channel changed it returns nothing, so the bytes wait.
- `Server::read` renews the decoder and greets whenever the number
  changes, and the closure given to `Host::run` returns as soon as
  `vsock::connection()` differs from the number it reads for.

Requests decoded from an ended connection before the change was seen
are still answered, on whatever channel is current (as before); a host
that dropped its connection is not waiting for them.

## Tests

`a_reconnect_between_two_polls_is_a_new_connection`
(`crates/rhizo-virtio/src/vsock/tests.rs`): through the simulated
device, a connection receives half a frame, then a reset, a request from
a new port and three bytes arrive before one `pump`. The stream is
connected before and after, the number changes, nothing is read with the
old number, the new connection's bytes are read with the new one, and
the new connection got its response. Against the code before this change
(the same packets, the old `read()`), the stream stays connected and the
reader that kept reading got `new` with nothing to tell it that the
connection had changed.

The kernel's wiring runs in the `control` test step (serial line where
the host has no `/dev/vhost-vsock`, vsock on CI's runners and on Cloud
Hypervisor); no step reconnects, since a reconnect inside one poll cannot
be timed from the host.
