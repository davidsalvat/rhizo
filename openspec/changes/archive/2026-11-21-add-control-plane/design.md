# Design

## D1. The split: the host decides, the kernel executes

The control kernel keeps a table of the cells it was asked to start and
does exactly what each request says: run this executable with these
arguments, environment and files; suspend, resume, fork, kill this cell;
stream its output; write this file into it; report memory and cells; power
off. It never chooses: which program a template means, which files it
gets, when an agent is suspended or forked, all live in `rhizo` on the host.
Executables and file sets are named, not sent: the kernel holds them (the
cells and the CPython files it embeds at build time), so a request stays
small and the kernel parses no executable from the host. Files the host
adds (`--file`) are sent and written into the cell's own files before it
starts.

The kernel gives a resumed snapshot a new cell identifier (`Host::resume`).
The control table hides that: a cell keeps the identifier it was started
with, and a fork's children get theirs. Killing a cell that ended forgets
it.

Suspended cells' snapshots stay in kernel memory, in the table; snapshots
on disk are later work (`docs/vision.md`).

## D2. Transport: the serial line, on every VMM

What each VMM offers a guest:

| VMM | UARTs | virtio-console | vsock |
|---|---|---|---|
| QEMU `microvm`, `q35` | 16550 (COM1) | yes (virtio-serial) | vhost-vsock, needs the host's `vhost_vsock` module |
| QEMU `virt` | one PL011 (non-secure) | yes | as above |
| Firecracker | one 16550 (x86_64: port 0x3f8; AArch64: MMIO) | no | yes (virtio-mmio, Unix socket on the host) |
| Cloud Hypervisor | one 16550 (x86_64) or PL011 (AArch64) | yes (virtio-pci) | yes (virtio-pci) |

The only device all of them have is the one UART, on the VMM's standard
input and output. virtio-console would leave Firecracker out; vsock needs a
virtqueue device with credit-based flow control and connection state, a
host module for QEMU (not loaded on CI runners), and differs per VMM on the
host side. The UART needs about 40 lines of kernel code: the receive side
of the 16550 and the PL011 drivers the console already has.

So the control channel is the serial line, both ways, everywhere. The
kernel's console text shares it: the protocol's frames start with the byte
0x01, which the kernel's text never contains, so the host tells them apart
(D3). On the VMMs where the console is a debug port (`console=e9`: QEMU
`microvm`, `q35`, Cloud Hypervisor on x86_64), a control kernel gets
`console=serial` instead (`RunOptions::control`). A cell's output never
reaches the console in a control kernel (it is captured and streamed in
frames), so nothing a cell prints can look like a frame.

Every VMM's serial input is lossless when the guest reads slowly: QEMU
offers the UART only what its FIFO can take; Firecracker stops reading its
standard input while the FIFO is full; Cloud Hypervisor queues input
without bound. The kernel drains the UART whenever it looks at it, and once
a frame has begun it reads the rest at once (D4).

Throughput is that of an emulated UART (one VM exit per byte with KVM):
fine for requests and an agent's output, not for bulk data. A vsock or
virtio-console transport can replace it per VMM later without changing the
protocol.

Alternatives that lost: a second UART (Firecracker and Cloud Hypervisor
have one);
QEMU's and Cloud Hypervisor's `-serial unix:` sockets (not Firecracker, and
`rhizo` would need two code paths anyway).

## D3. Protocol (`rhizo-control`)

```
0x01 'Z' version kind id:u32 length:u32 payload[length]     (little endian)
```

- Version 1. A frame of another version is refused (with its identifier)
  and skipped by its length.
- `kind`: requests 0x01 to 0x0a (run, list, suspend, resume, fork, kill,
  output, write-file, stats, shutdown); messages 0x80 to 0x84 (hello, done,
  failed, output, ended).
- `id`: chosen by the requester, returned in the reply, and in every output
  and ended message of an `output` request. Hello has 0.
- Payloads: `u8`, `u32`, `u64`, byte strings (`u32` length + bytes), lists
  (`u32` count + items). `done` and `failed` start with the kernel's time
  in nanoseconds.
- The decoder works a byte at a time and returns text bytes, frames, or a
  refusal. Text never holds 0x01, so a frame is found after any amount of
  text or damage. A payload larger than 1 MiB is refused and its length
  not trusted (the decoder looks for the next frame). The payload buffer is
  reserved with `try_reserve`, so a large valid length cannot panic the
  kernel; list counts are bounded by the bytes left, so a forged count
  allocates nothing. Decoding checks every length and rejects trailing
  bytes.
- Host tests round-trip every request and message, separate text from
  frames, refuse other versions, oversized and malformed payloads;
  property tests round-trip arbitrary requests and messages between
  arbitrary text, feed arbitrary bytes and arbitrary payloads under every
  kind (no panic), and damage one byte of a frame (no panic, and the next
  frame is found whenever the damage leaves the header readable). The
  channel is an attack surface: a compromised host process, or a bug in
  `rhizo`, must not crash the kernel.

The kernel crate depends on `rhizo-control` for decoding and encoding; the
logic it adds itself (`kernel/src/control.rs`) is the table and the calls
into `Host`.

## D4. Serving without interrupts

The control kernel's boot CPU runs `Host::run` with a stop condition that
drains the UART into the decoder and stops when a frame is complete (or
begun), when a cell ended, or, while someone streams output, every 10 ms.
`Host::look_every` makes the boot CPU's waits (nothing to run, or every
cell asleep) last at most a given time, so the UART is looked at even when
every cell sleeps for an hour: 50 µs for 200 ms after a request, then 2 ms.
That bounds the latency of a request on an idle VM by 2 ms, and keeps it
near the UART's own while the host is talking; an idle control kernel
wakes 500 times a second (measurable, not free: UART interrupts are later
work).

While a cell computes without blocking, requests wait for the end of its
time slice.

## D5. `rhizo` and `xtask`: one library

`rhizo up` must build the control kernel (cells, CPython, flags per
architecture), launch QEMU, Firecracker or Cloud Hypervisor with the right
devices and command line, download Firecracker and Cloud Hypervisor, and
create tap devices: all of which `xtask` does. Rather than move that code
into a third crate (and every `xtask` module that uses it with it), `xtask`
becomes a library with a thin binary (`src/main.rs` calls
`xtask::main()`), and `rhizo-cli` depends on it. `xtask` stays host-only
tooling; `rhizo` is a developer tool of the proof of concept that builds
its kernel from the checkout. A product would ship a prebuilt kernel and
drop the dependency.

`rhizo up` starts a background server (`rhizo __serve`, own process group,
output in `server.log`) that owns the VMM: its standard input and output
are the serial line. The server relays frames between the kernel and the
clients of a Unix socket, giving each request an identifier of its own so
replies and output go back to whoever asked (several terminals can attach,
fork and kill at once). The kernel's console text goes to `console.log`.
The state directory (`--state`, `$RHIZO_STATE`, `~/.rhizo`) holds the
state file (server and VMM processes, VMM, architecture, memory, CPUs,
boot time), the socket, the logs and the kernel image the VM booted. When
the VMM ends, the server removes the state and the socket. `rhizo down`
asks the kernel to power off and waits for that.

The default architecture is the host's (CPython is built natively); the
default VMM that of the architecture, as in `xtask`.

## D6. The agent through the control plane

`rhizo run python-agent --llm URL --model NAME [--turns N]` runs the
CPython agent of `add-agent-workload` (`image cpython`, files
`cpython-files`, `agent_loop.py URL MODEL cell N live`), with `URL`'s
loopback rewritten to the host as the guest sees it on the VM's VMM (as
`xtask demo agent` did). It streams the agent's output and, as the host:

- after each line that ends a turn, suspends and resumes the agent and
  prints both times and the snapshot's size (a suspend may land while the
  next request is in flight: the connection is parked, `keep-tcp-across-
  suspend`);
- at the decision point, forks the agent into three cells, writes each its
  branch number into `/branch`, streams them, and kills the agent;
- prints each branch's lines, and a final `rhizo-demo done` line with the
  median suspend plus resume and the fork, as the kernel timed them.

The agent used to sleep for an hour at its decision point, which only a
template (whose children's timers expire) woke. It now looks for `/branch`
every 10 ms: a plain fork's children find it as soon as the host writes it,
and templates work as before. `--transcript` runs the agent without
`live` (its deterministic transcript, branches printed in order), which
the `control` test step checks against `cells/expected.rs`.

`cargo xtask demo agent` builds the control kernel and `rhizo`, and runs
`rhizo up`, `rhizo run python-agent` and `rhizo down`. The benchmark
kernel's `bench=agent-demo` and the kernel's live agent output, now
unused, are removed.

## D7. Tests and metrics

- `cargo xtask test --only control` (part of `cargo xtask test`, so of CI
  on x86_64 and AArch64, on QEMU): builds the control kernel and `rhizo`,
  then `rhizo up`, `run hello` (output and exit), an idle cell through
  `suspend`, `ps`, `resume`, `fork 2`, `kill`, `ps`, `kill` (forget), a
  refused unknown cell, the agent with `--transcript` against
  `hostnet::llm_server` (transcript equal to `AGENT_TRANSCRIPT`, a suspend
  per turn, the fork), `stats` and `down`. Without CPython the agent part
  says it did not run.
- `cargo xtask bench`: per VMM, a control kernel answers 200 `list`
  requests (`control.roundtrip_us`, the host's round trip) and 20 rounds of
  `run` of the idle cell, `suspend`, `resume`, `fork 1` and `kill` as the
  host times them (`control.run_us` and the others). Gated in `ci` like
  every timing.
