# Design

## D1. The transport: virtio-vsock

`measure-control-path` (D4) showed where a request's time goes: on the
serial line every byte in costs two trapped register accesses (line
status, data) and every byte out one, 35 to 40 µs a byte on the CI
runners, so a spawn to first output (~150 bytes) took 3 to 4 ms for a
kernel that spends 3 µs. The channel has to cost per frame, not per byte.

virtio-vsock does: the driver puts a packet in a ring and notifies the
device once; packets from the host arrive in buffers the driver gave,
found by reading the used ring (memory, no trap). Every VMM the project
runs has it: Firecracker and Cloud Hypervisor implement it in their own
process behind a Unix socket, QEMU through the host kernel's
`vhost-vsock`. Alternatives that lost:

- **virtio-console**: Firecracker has none.
- **A shared-memory ring with a doorbell of our own**: no VMM offers it.
- **Faster serial (FIFO bursts, as D3 of `measure-control-path`)**:
  receiving cannot burst on a 16550, and the cost stays per byte.

The protocol's frames do not change: the stream carries exactly what the
serial line carried, without the console's text, so `rhizo-control`, its
version and every request stay as they are.

## D2. The kernel: one stream, polled

`rhizo-virtio` gets the device beside the network card. What the two
share moved out of `VirtioNet`: the virtio-mmio check (`mmio`, a device
type given), feature negotiation (`negotiate`), queue set-up (`queue`),
descriptors that may be chained, and PCI functions of any type
(`pci::regions`, `pci::is_virtio`, modern device ID `0x1040 + type`).
The vsock part is two pieces:

- `vsock::Stream`, the protocol with no device: the 44-byte packet header;
  one connection, made by a `REQUEST` to the listening port; credit both
  ways; resets. It is plain data driven by `take` (a packet received),
  `read`, `write` and `next_packet`, so the host tests drive it alone,
  with property tests of hostile packets.
- `vsock::VirtioVsock`, the device: receive, transmit and event queues on
  either transport, polled like the network card (no interrupts asked).

The control kernel (`kernel/src/vsock.rs`, `control` builds only) brings
up the first socket device it finds, listens on port 1024
(`rhizo_control::VSOCK_PORT`), and is polled from the control loop where
the loop used to read the serial line: a look reads the used ring, which
costs no trap.

One connection at a time: the host side holds one (the `rhizo` server, or
`xtask`), and a second request is reset rather than taking the channel
from the first. When a connection ends (reset, shutdown) the kernel goes
back to the serial line, and a new connection is accepted.

## D3. Switching channels

The kernel boots as before and says hello on its serial line: that is how
the host knows it booted, on any VMM, and the whole protocol still works
there when the VM has no socket device. The host then connects to the
vsock port; the kernel says hello again on the connection, and from then
on reads requests and writes replies and output only there. Its decoder
starts afresh on each switch, so a frame half read on one channel is not
completed from the other. Console text (boot messages, panics) stays on
the serial line, which the host keeps reading into the console log.

On the host, `xtask::control::Vm` (which `rhizo up`'s relay, the
benchmarks and the `control` test step all use) waits for the serial
hello, connects, starts a reader of the stream that feeds the same event
channel as the serial reader, points its `Writer` at the stream, and
waits for the second hello. The relay (`cli/src/serve.rs`) does not
change: it gets frames from `Vm` and writes through `Writer` as before.

Alternative that lost: the guest connecting to the host. Firecracker and
Cloud Hypervisor would then need a listener on `<socket>_<port>` and QEMU
an `AF_VSOCK` listener on the host, and a restarted host side could not
reconnect to a running VM.

## D4. Each VMM

`vmm::Vsock::for_vmm` decides, for control kernels only (other runs get
no device, so no boot or benchmark VM changes):

| VMM | Device | Host side |
|---|---|---|
| Firecracker | its vsock (`"vsock": {"guest_cid", "uds_path"}`) | Unix socket, `CONNECT 1024\n`, answer `OK <port>\n` |
| Cloud Hypervisor | its vsock (`--vsock cid=N,socket=PATH`), virtio-pci | the same |
| QEMU `microvm`, `virt` | `vhost-vsock-device` (virtio-mmio) | `AF_VSOCK` socket to `(cid, 1024)` |
| QEMU `q35` | `vhost-vsock-pci` | the same |

- The context identifier is unique on the host (QEMU's vhost requires it):
  3 plus the process identifier shifted by 6, plus a counter of the
  process's VMs. The Unix socket is in the temporary directory, whose
  short path stays under the 108 bytes of a socket address.
- QEMU needs `/dev/vhost-vsock` open for reading and writing. GitHub's
  `ubuntu-24.04` and `ubuntu-24.04-arm` runners have it, `root:kvm`
  `0660`, and password-less `sudo` (checked by a probe run). As for tap
  devices (`hostnet`), `xtask` then runs `sudo -n modprobe vhost_vsock`,
  `udevadm settle` and `chmod 0666`, in that order: loading the module on
  the first open made udev put `0660` back between our `chmod` and QEMU's
  open (seen on both runners in the first probe run). Where the device is
  missing or `sudo` is not password-less (the development container this
  was written in: no `/dev/vhost-vsock`, no KVM), the channel stays the
  serial line, and `bench` prints which channel each VMM used.
- `AF_VSOCK` is not in Rust's standard library: `xtask` takes `libc`
  (MIT OR Apache-2.0, already in the lock file through other crates) for
  `socket` and `connect`, in one function with `// SAFETY:` comments.
- A vhost-user daemon (`vhost-device-vsock`, Apache-2.0 OR BSD-3-Clause)
  would give QEMU a vsock without `/dev/vhost-vsock`, behind the same Unix
  socket handshake as Firecracker. Not done: a separate program to
  install and run beside every VM, for hosts CI does not have.
- If the host was given a device and the connection or the second hello
  fails, starting the VM fails with the reason: the device was there, so
  something is broken, and falling back would hide it.

## D5. Cloud Hypervisor never answered

Root cause: Cloud Hypervisor's serial line in `tty` mode reads the VMM's
standard input only when it is a terminal. In v54
(`vmm/src/serial_manager.rs`, `SerialManager::new`), the `Tty` transport
takes input only `if isatty(STDIN_FILENO)`; otherwise no serial manager
is created and input is dropped. `xtask` gives the VMM a pipe, so the
control kernel's hello reached the host (output works) and no request
ever reached the kernel: "no reply to List".

Fix: on Cloud Hypervisor, a control kernel's frames take its vsock device
(D4), which Cloud Hypervisor always provides; the serial line keeps the
console's text. Alternatives that lost: `--serial socket=PATH` or `pty`
(input would work, but at the serial line's cost, and with a second kind
of host reader). Tests: `vmm` host tests check that control kernels on
Cloud Hypervisor and Firecracker get a socket device; the `control` step
passes on Cloud Hypervisor (probe run), and the benchmark reports its
control figures.

## D6. The device in detail

- **Packets as two descriptors**: a 44-byte header descriptor chained to a
  2 KiB payload descriptor, in both directions; a packet with no payload
  is its header alone. Every implementation accepts this layout (older
  Firecracker parsers and Cloud Hypervisor's insist on the header in a
  descriptor of its own; vhost and newer Firecracker take any layout).
  Queues of 64 descriptors hold 32 packets.
- **Event queue**: enabled, no buffer offered. Firecracker activates a
  device only when all its queues are ready; the only event, a transport
  reset after a VM is restored, does not happen to a control kernel.
- **Memory**: 99 frames (396 KiB) for the three queues, control builds
  only.
- **Credit**: the stream advertises 256 KiB and never holds more unread
  (a peer that sends past its credit is reset); it never sends past the
  peer's credit, asks for credit once when it has bytes and none, and
  tells the peer what it read once it read half its buffer (every packet
  also carries it). More than 4 MiB written and not sent (a host that
  stopped reading) resets the connection, so a host cannot make the
  kernel buffer without bound.
- **Untrusted input**: a used buffer shorter than a header is skipped, a
  length beyond the buffer clipped, a descriptor out of range folded into
  the ring (no buffer is read or written outside the driver's own);
  packets for another port, from another peer or of another type get a
  reset (never in answer to a reset); a length that is not its payload's
  resets the connection; unknown operations are ignored. Control packets
  waiting to go out are capped (64), so a flood cannot grow memory.
- **Notifications**: a packet sent is one notification (one trap); a
  packet received costs a notification when its buffer goes back.
  A request and its reply are about two traps whatever their size,
  against ~75 for a `run` of `hello` on the serial line.

## D7. Kernels without a control channel

The device code is compiled into control kernels only: plain, test and
benchmark kernels do not call it, and the generic driver is not
instantiated there. Two things kept them from growing:

- Discovery lives in `kernel/src/vsock.rs` rather than shared with the
  network card's in `wire.rs`: sharing it (one generic `find`) grew the
  x86_64 benchmark kernel's code by 1.9 KB.
- The PCI transport keeps each queue's `queue_notify_off` (`u16`) and
  multiplies when it notifies, instead of caching the offset (`usize`):
  with a third queue the cache grew the network card's state, a static of
  the benchmark kernels, by 8 bytes.

Release kernels against `main` (`3e59e35`), loadable section bytes
(`.text`, `.rodata`, `.data`, `.bss`):

| Kernel | `main` | This change |
|---|---|---|
| x86_64 plain (`image.size_bytes`: 47,784 bytes, unchanged) | 273,312 | 273,312 |
| x86_64 `bench-cell` | 6,613,760 | 6,613,720 |
| x86_64 `control` | 6,393,648 | 6,410,704 (+17 KB: the driver) |
| AArch64 plain | 286,720 | 286,720 |
| AArch64 `bench-cell` | 5,629,240 | 5,629,232 |
| AArch64 `control` | 5,399,864 | 5,416,632 (+17 KB) |

PCI BARs: a socket device on PCI is found only where the firmware or the
VMM assigned its BARs (`q35`, Cloud Hypervisor); the control kernel never
gets a PCI vsock on QEMU `virt`, whose BARs the kernel assigns for the
network card.

## Measurements

`cargo xtask --arch x86_64 bench --workload control`, after then before
(`main` at `3e59e35`), on the same GitHub `ubuntu-24.04` runner (KVM, 1
vCPU, 128 MiB), run 44936943897 of a probe workflow; µs, medians:

| Figure | VMM | Before | After |
|---|---|---|---|
| `control.spawn_to_first_output_us` | `qemu-microvm` | 4270 | **246** |
| | `qemu-q35` | 4409 | **241** |
| | `firecracker` | 3845 | **219** |
| | `cloud-hypervisor` | none | **245** |
| `control.fork_to_first_output_us` | `qemu-microvm` | 3038 | **237** |
| | `qemu-q35` | 3152 | **236** |
| | `firecracker` | 2821 | **216** |
| | `cloud-hypervisor` | none | **249** |
| `control.roundtrip_us` | `qemu-microvm` | 1061 | 114 |
| | `qemu-q35` | 1017 | 112 |
| | `firecracker` | 1001 | 104 |
| | `cloud-hypervisor` | none | 112 |
| `control.run_us` | `qemu-microvm` | 2356 | 114 |
| | `firecracker` | 2083 | 120 |
| `control.suspend_us` | `qemu-microvm` | 1227 | 123 |
| `control.fork_us` | `qemu-microvm` | 1458 | 118 |
| `chat.turn_us` | `qemu-microvm` | 5169 | 697 |
| | `firecracker` | 4462 | 719 |
| | `cloud-hypervisor` | none | 707 |
| `breakdown.control.idle_roundtrip_us` | `qemu-microvm` | 1229 | 327 |

Every `control.` figure is now 104 to 126 µs per request on every VMM
(each was 1.0 to 2.4 ms and grew with the frame's size); spawn and fork to
first output are 0.22 to 0.25 ms, inside the 0.2 to 0.4 ms aimed at. The
bytes no longer count: `serial_in_ns_per_byte` and
`serial_out_ns_per_byte` (now the channel's cost per byte; the names are
kept so the series continue) dropped from 42,525 and 17,141 ns to 15 and
0.5 ns on `qemu-microvm`. The kernel's share is unchanged (3.1 µs to spawn
`hello`, 0.8 µs to fork, 0.04 µs for a `list`). The snapshot figures,
whose transfers take the network card, did not move beyond noise (save
68.2 ms before, 63.9 after; load 31.5, 32.0), and their answers after a
load, which go through the control channel, became faster (12.1 to
2.4 ms).

Where a round trip's ~110 µs go now: the kernel looks at its channel
every 50 µs after a request (half of that waited on average, each way a
request crosses), the timer interrupt that wakes it, and the host's and
the VMM's threads waking for the packet (vhost worker, Firecracker's
event loop). It was not broken down further.

## D8. Follow-ups (recommended, not in this change)

1. **Stream a new cell's output in its `run` or `fork` reply.** First
   output still takes two round trips (start, then `output`), each
   ~110 µs: a watch flag on `run` and `fork` would send the output frames
   right after the reply, halving spawn and fork to first output (to
   ~120 µs). It needs a protocol version from the coordinator. Clearly
   worth it now that a round trip, not the bytes, is the cost.
2. **Wake on arrival.** The device can interrupt the idle CPU when a
   packet arrives, instead of the 50 µs look (and the 2 ms look after
   quiet, the idle round trip's 0.3 to 0.45 ms): it would remove most of
   what is left of a round trip on the guest's side.
3. A vhost-user backend for hosts without `/dev/vhost-vsock` (D4), if
   such hosts matter.

## Baselines

`bench/baseline.json` gains, in `ci`, the control figures of
`cloud-hypervisor`, which had none; the existing `control.` values of
`qemu-microvm`, `qemu-q35` and `firecracker` are lowered to what this
change measures, so that a return to the serial line would fail the gate.
`chat.turn_us`, which crosses the same channel, is lowered (and added for
`cloud-hypervisor`) the same way. No value rises. The values are the mean
of the medians of two `bench` workflow runs of this branch's commit
(`0ad5374`), runs 44938481371 and 44938483458, which both passed the `ci`
gate and agree within a few percent:

| Figure, µs | VMM | Run 44938481371 | Run 44938483458 |
|---|---|---|---|
| `control.spawn_to_first_output_us` | `qemu-microvm` | 240 | 242 |
| | `qemu-q35` | 243 | 250 |
| | `firecracker` | 234 | 243 |
| | `cloud-hypervisor` | 251 | 246 |
| `control.fork_to_first_output_us` | `qemu-microvm` | 236 | 237 |
| | `qemu-q35` | 236 | 247 |
| | `firecracker` | 231 | 232 |
| | `cloud-hypervisor` | 249 | 252 |
| `control.roundtrip_us` | `qemu-microvm` | 112 | 113 |
| | `qemu-q35` | 112 | 112 |
| | `firecracker` | 104 | 104 |
| | `cloud-hypervisor` | 108 | 112 |

The `ci` threshold (100 %) still leaves room for a slow runner, and fails
a return to the serial line (ten times slower).

AArch64 has no KVM in CI; under emulation (`qemu-virt`, TCG, the
`ubuntu-24.04-arm` runner's `vhost-vsock`) a round trip took 109 µs and a
spawn to first output 352 µs, where the serial line took milliseconds.
