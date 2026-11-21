# Design

## The path

A request from the host reaches a cell and its output comes back through:

1. the host process: `rhizo` writes a frame to the background server's
   Unix socket, which relays it to the VMM's standard input
   (`cli/src/serve.rs`); the benchmark and `xtask` write to the VMM's
   standard input directly (`xtask/src/control.rs`);
2. the VMM: its emulated 16550 (x86_64) or PL011 (AArch64) receives the
   bytes;
3. the serial line in the guest: the control kernel polls the line
   status register and reads the receive register, one byte at a time
   (`kernel/src/arch/*/console.rs`);
4. the kernel: it decodes the frame and runs the request
   (`kernel/src/control.rs`); `run` spawns the cell, which starts at
   once;
5. back: the reply frame, written a byte at a time to the transmit
   register; then the host's `output` request (it needs the cell's
   identifier from the reply) and the cell's output frame, the same way.

## D1. What is timed

The host times both figures, from writing the request that starts the
cell until it has read the first output frame of that cell:

- `control.spawn_to_first_output_us`: `run` of `hello` (static, kept
  loaded after its first spawn), then `output` of the cell it names.
- `control.fork_to_first_output_us`: `fork` into one of a template, then
  `output`. The template is `sleeper 3600000 yield` frozen in its hour
  of sleep: a child of a template starts with its deadlines expired
  (spec `fork`), so it prints `woken` and exits at once. The host waits
  100 ms after `ready` before `freeze`: with requests arriving, the
  kernel serves them before running cells, and a template frozen before
  its sleep would have children sleep the hour (seen under TCG).

Each is the median of 20, after the existing `control.` figures, on the
same boot; every cell is waited to its end and forgotten (`kill` of an
ended cell) before the next. The no-op request is the existing
`control.roundtrip_us` (a `list`, median of 200).

Two requests (start, then `output`) are what the protocol offers today:
the host cannot name the cell to watch before the kernel gives its
identifier. That is the path users take (`rhizo run` does the same), so
it is what is measured; D4 says how to save the second round trip.

## D2. The breakdown

Ungated, under `breakdown.control.` (`breakdown.` is in every
environment's `ungated` list):

- for each start, the start request's round trip (`*_reply_us`), the
  kernel's own time for it as its reply carries it (`*_kernel_us`) and
  from the `output` request to the first output (`*_watch_us`);
- `roundtrip_kernel_us`: the kernel's time for a `list`;
- `serial_in_ns_per_byte` and `serial_out_ns_per_byte`: requests the
  kernel refuses at once, timed against a `list`: a `write-file` of
  1 KiB to no cell carries 1 KiB in; a `run` of an executable whose
  1 KiB name the refusal repeats carries 1 KiB in and 1 KiB out. Their
  differences, over 1 KiB, are the cost of a byte each way. 1 KiB keeps
  a probe under 50 ms on CI;
- `idle_roundtrip_us`: a `list` after 300 ms of quiet, when the kernel
  looks at the line every 2 ms instead of every 50 µs (median of 10).

The frame sizes are fixed by the protocol (12-byte header; a `list` is
12 bytes in and its empty reply 25 out; `run` of `hello` 47 in, its
reply 25 out; `output` 16 in, its `done` 21 and the output frame
20 + the bytes printed out), so bytes times cost per byte predicts each
figure.

## Measurements

`bench` workflow runs of this branch on GitHub's `ubuntu-24.04`
runners (x86_64, KVM inside a VM: nested virtualization), 1 vCPU,
128 MiB. Before the change of D3: run 44929147780; after: runs
44930748675 and 44931429957 (same commit). Runners differ from run to
run (the same code gave `control.roundtrip_us` = 872 µs on main's run
44927008397 and 1459 µs in run 44929147780), so compare within a run
first: the kernel's share, the bytes' share, and out against in.

Before (run 44929147780), `qemu-microvm`, µs:

| Figure | Bytes in | Bytes out | Bytes × cost | Measured |
|---|---|---|---|---|
| `control.roundtrip_us` (`list`) | 12 | 25 | 1363 | 1459 |
| `control.kill_us` | 16 | 21 | 1381 | 1465 |
| `control.fork_us` | 20 | 29 | 1823 | 1909 |
| `control.run_us` (`idle`) | 45 | 25 | 2678 | 2756 |
| `control.spawn_to_first_output_us` | 63 | 84 | 5483 | **5516** |
| `control.fork_to_first_output_us` | 36 | 76 | 4123 | **4200** |

with a byte costing 39.9 µs in and 35.4 µs out. Firecracker and
`qemu-q35` are alike (spawn to first output 5823 and 6422 µs; bytes
explain 92 to 100 % of every figure on all three). Where the rest goes:

| Part of spawn to first output (`qemu-microvm`) | µs | Share |
|---|---|---|
| Serial line: 63 bytes in, 2 register reads each (line status, data) | 2512 | 46 % |
| Serial line: 84 bytes out, a status read and a write each | 2971 | 54 % |
| Kernel: decode, spawn `hello` (`spawn_kernel_us`), `list` 0.1 µs | 3.3 | 0.06 % |
| Host process, VMM wake-ups, the cell's run, polling (the remainder) | ~30 | ~0.5 % |

Fork to first output: the kernel's part is 1.0 µs. An idle round trip
(1527 µs) costs about as much as a busy one (1459 µs), and 617 against
687 µs after D3: the 2 ms slow look is not what a user waits for today
(why the idle kernel answers as fast was not investigated; it matters
once the line is fast, D4).

The `rhizo` relay (`cli/src/serve.rs`: Unix socket, a thread per client,
the VMM's reader thread, one copy and re-encoding per frame) adds about
50 µs per round trip, measured locally (TCG, no KVM) as a `list` through
the socket against the same request written to the VMM directly; it is
host work only, so the same on CI, under 10 % of a round trip today.

**Bottleneck: the serial line**, 99 % of the path. Each byte costs two
accesses to UART registers, and on KVM each access is a VM exit to the
VMM in user space: about 18 to 20 µs on these nested runners. Bare
metal was not measured; an exit there costs microseconds, still far
above a kernel that spends 3 µs.

## D3. Cheap win: transmit bursts on x86_64

With its FIFOs on (`Serial::init` writes `0xC7` to FCR), a 16550 whose
line status says "transmitter empty" takes 16 bytes. The control frames'
writer (`write_raw`, control builds) now reads the line status once per
16 bytes instead of before every byte: out, a byte costs one exit
instead of two. Without FIFOs (a console never initialized) it keeps
looking before every byte. Sound on every x86_64 VMM: QEMU's 16550 has a
16-byte transmit FIFO; Firecracker's and Cloud Hypervisor's write each
byte out at once and always report the transmitter empty.

Only control frames burst. The console's own text keeps its
byte-at-a-time writer: bursting there too grew the default release
image by 88 bytes, past the 1 % gate on `image.size_bytes` (run
44929940604), for text nobody times; the default image is unchanged
(47,784 bytes, as on main).

After: runs 44930748675 (a fast runner: a byte in cost 31 µs on
`qemu-microvm`) and 44931429957 (a slow one, 44 µs, like the run
before at 40 µs). The change does not touch bytes in, so out against in
within a run shows it whatever the runner:

| VMM | Out/in per byte: before; after (two runs) |
|---|---|
| `qemu-microvm` | 0.89; 0.46 and 0.51 |
| `qemu-q35` | 0.92; 0.49 and 0.52 |
| `firecracker` | 1.05; 1.08 and 0.53 |

and the figures, µs, before; after on the fast runner and on the slow one:

| VMM | Spawn to first output | Fork to first output | `control.roundtrip_us` |
|---|---|---|---|
| `qemu-microvm` | 5516; 3086, 4327 | 4200; 2044, 3067 | 1459; 687, 1074 |
| `qemu-q35` | 6422; 3673, 4886 | 4905; 2392, 3477 | 1664; 801, 1225 |
| `firecracker` | 5823; 2409, 4356 | 4521; 1889, 3181 | 1560; 537, 1134 |

A byte out now costs half a byte in, where it cost as much: the change
removes nearly half of the exits for the bytes the kernel sends (84 of
the 147 bytes of a spawn to first output). On the slow runner, against
the run before on a similar one, spawn to first output drops by 22 %
and a round trip by 26 % on `qemu-microvm`, although bytes in cost 9 %
more there; bytes still explain 94 to 107 % of every figure. One
Firecracker result (1.08 in run 44930748675) did not improve where the
other did (0.53); nothing in the code differs between them, so it is
counted as noise of that runner. No other metric moved beyond the `ci`
gate (both runs passed it).

Not done, and why:

- Receiving cannot burst: a 16550 says a byte is waiting only in the
  line status, so every byte in costs two exits.
- AArch64 is unchanged. Its PL011 runs with FIFOs off (`LCR_H.FEN` = 0,
  as the firmware-less boot leaves it), so its holding register takes
  one byte; turning the FIFO on changes receiving too, and AArch64 has
  no KVM timing in CI to show the effect (bench/README.md). Firecracker
  on AArch64 gives a 16550 whose FIFOs the kernel does not turn on. The
  transport of D4 replaces both.

## D4. Recommendation: `control-over-vsock`

After D3 a spawn to first output still crosses the line as ~150 bytes,
i.e. ~200 exits; the kernel's 3 to 5 µs are lost in it. The follow-up
change should move the control channel to **virtio-vsock**:

- cost per frame, not per byte: the guest posts a frame into a virtqueue
  and notifies once (one exit); frames from the host arrive by DMA, seen
  in the used ring by the same polling the network card gets (no exit)
  or by an interrupt. A request and its reply become 2 to 3 exits
  instead of ~50 to 75, whatever their size, which also fixes large
  frames (a 1 KiB `write-file` costs ~32 ms on the line today);
- the transport every VMM has: Firecracker offers vsock (and no
  virtio-console), Cloud Hypervisor and QEMU (`vhost-vsock`) too; the
  host side is a Unix socket on Firecracker and Cloud Hypervisor, which
  the `rhizo` server can relay as it relays the serial line today;
- `rhizo-virtio` already has both transports (mmio, pci) and split
  virtqueues; vsock adds a device beside the network card's;
- the protocol's frames stay as they are; console text stays on the
  serial line, which removes the need to tell frames from text there.

Expected on these runners: a round trip near the host's and VMM's own
cost (~50 to 100 µs), so spawn to first output ~0.2 to 0.4 ms instead
of 3.1 to 4.3 ms, against 717 ms for E2B and under 50 ms for
Freestyle's fork.

Then, in the same or a next change (each needs a protocol version from
the coordinator):

- **one round trip, not two**: a `run` or `fork` that also streams the
  new cell's output (a watch flag), so the first output follows the
  reply without a second request; with vsock that halves the path;
- **wake on arrival**: once the serial line's cost is gone, the kernel's
  polling becomes visible (50 µs after a request, up to 2 ms after
  quiet); the vsock device's interrupt can wake the idle CPU instead.

Known issue, unchanged here: on Cloud Hypervisor the control kernel
never answers its first `list` (`control/cloud-hypervisor` is skipped
on main with "no reply to List"), so no control figure exists there.

## Baselines

`bench/baseline.json` gains, in `ci` only, the two gated figures on
`qemu-microvm`, `qemu-q35` and `firecracker`: the mean of the medians
of runs 44930748675 and 44931429957 (the code as merged), which agree
within the `ci` threshold (the slow runner's figures are 1.3 to 1.8
times the fast one's). No existing value changes; the breakdown stays
ungated. The existing `control.` figures improve, so their baselines
need no update now (lowering them is left to a baseline refresh).
