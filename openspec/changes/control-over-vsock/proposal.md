# Proposal

## Why

`measure-control-path` found the serial line to be 99 % of the control
path: every byte a request or a reply carries costs one or two trapped
register accesses, about 35 to 40 µs a byte on the CI runners, so a spawn
to first output takes 3.1 to 4.3 ms on `qemu-microvm` while the kernel
spends 1 to 5 µs of it. Its design (D4) recommends moving the control
channel to virtio-vsock, where a frame costs a notification, not a trap per
byte: the target is a spawn to first output of about 0.2 to 0.4 ms on the
same runners.

Cloud Hypervisor has no control figures at all: its control kernel never
answers the first `list`. Cloud Hypervisor's serial line takes input only
when the VMM's standard input is a terminal; the host pipes it, so the
kernel never sees a request.

## What Changes

- **A virtio socket device driver** in `rhizo-virtio`: the device on the
  virtio-mmio and modern virtio-pci transports (receive, transmit and event
  queues, packets as a header descriptor chained to a payload one), and,
  apart from the device, one stream socket: packet header, the connection
  a peer requests on the listening port, credit-based flow control both
  ways, resets, and checks of every packet as untrusted input. Host tests
  against a simulated device, and property tests of hostile packets.
- **The control kernel listens on vsock port 1024** when its VM has a
  socket device (`kernel/src/vsock.rs`, `control` builds only). Once the
  host connects, the kernel says hello on the connection and its frames
  take it; until then, or without a device, they take the serial line as
  before. The console's text stays on the serial line. Plain, test and
  benchmark kernels do not get the driver.
- **The host connects over vsock** (`xtask/src/control.rs`, used by
  `rhizo up`'s relay, the benchmark and the `control` test step): after
  the hello on the serial line it connects, waits for the hello on the
  stream, and sends every request there. Firecracker and Cloud Hypervisor
  get their own vsock device (a Unix socket on the host with a
  `CONNECT <port>` handshake); QEMU (`microvm`, `q35`, AArch64 `virt`)
  gets `vhost-vsock` when the host's `/dev/vhost-vsock` is usable
  (`xtask` makes it so with `sudo -n` where it can, as for tap devices),
  else the serial line stays the channel. No new cargo feature.
- **Cloud Hypervisor answers**: its control kernel's frames take the vsock
  device, so the `control` step and the control benchmarks run there.
- The protocol, its frames and its version do not change.

## Out of scope

- Streaming a new cell's output in the reply to `run` or `fork` (one round
  trip instead of two): it needs a protocol version; recommended as a
  follow-up in the design.
- Waking the idle kernel on a packet's arrival (an interrupt from the
  device) instead of its periodic look.
- A vhost-user backend for hosts without `/dev/vhost-vsock`.

## Capabilities

- Modified: `control-plane` (the control channel on every VMM).
- Added to `control-plane`: the vsock stream's handling of hostile packets.

## Impact

- Code: `crates/rhizo-virtio` (`vsock.rs`; queues and transports shared
  with the network card, PCI functions of any device type),
  `crates/rhizo-control` (`VSOCK_PORT`), `kernel/src/vsock.rs`,
  `kernel/src/control.rs`, `xtask/src/vmm.rs`, `xtask/src/control.rs`.
- Dependency: `libc` in `xtask` for `AF_VSOCK` sockets (MIT OR Apache-2.0).
- Benchmarks: every `control.` figure drops; `cloud-hypervisor` gets
  control figures (new `ci` baseline entries); no existing baseline
  value rises. Plain and benchmark kernel images do not grow.
