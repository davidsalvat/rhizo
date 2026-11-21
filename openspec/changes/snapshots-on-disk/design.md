# Design

## D1. A file is the exported snapshot plus a CRC-32C

A snapshot file is exactly `Snapshot::export`'s output: the snapshot's
bytes with the contents of the frames it shares written in place (design
D2 of `add-shared-snapshots`), followed by the CRC-32C of everything before
it (4 bytes, little endian). Nothing wraps it: the snapshot's own header
already says what it is (magic), which format it is (version) and which
architecture took it (ELF machine).

`from_bytes` checks, in order: the header (magic, version, machine), the
checksum, then everything the snapshot holds, as before. The order gives
the clearest reason: a file of another version or architecture says so
(`snapshot format version 8; this kernel reads version 9`, `taken on
aarch64; this kernel runs on x86_64`) rather than a checksum mismatch,
while any other damage, a cut file included, reads `checksum mismatch:
the file is damaged or truncated`. The checksum is stripped once checked,
so a decoded snapshot is byte for byte what `capture` writes. In-kernel
snapshots never carry it: `suspend` and `resume` pay nothing.

Page size needs no field: every page record says whether it is 4 KiB or
2 MiB, and both architectures use a 4 KiB granule. A kernel with another
page size, or any other change to what a snapshot means, is a new format
version, which older kernels refuse by its number.

`export` now takes `&self`: a suspended cell, or the snapshot of a
template, stays as it was after a save. The references to shared frames
are dropped with the snapshot, no longer by the export.

Alternative: SHA-256. Rejected: it needs a dependency or 100 lines of our
own, and is 5 to 10 times slower than a table CRC on a 15 MiB file in a
`no_std` kernel without SHA instructions in use, for no gain here: the
checksum detects damage in transit or on disk; it cannot detect forgery,
since whoever forges the bytes recomputes any unkeyed hash. Forged bytes
are contained by validation instead (spec `isolation`, D4). Authenticated
files (a signature checked against a key the host trusts) are follow-up
work, and would sign the whole file whatever its checksum.

Alternative: CRC-32 (IEEE). Equivalent; CRC-32C has the better error
detection at these lengths and hardware support on both architectures if
it is ever needed.

## D2. Transport: the serial line, in chunks of 64 KiB

Requests and replies already travel in frames of at most 1 MiB on the
serial line. Snapshot bytes travel in the same frames, in chunks of
64 KiB (`rhizo_control::CHUNK`):

- `save <cell>`: the kernel exports the snapshot into its heap (refused
  when the heap cannot hold it), sends it as `data` frames carrying the
  request's identifier, then `done` with the total. The relay keeps a
  `save`'s route open until then.
- `load <size> <template>`: the kernel reserves `size` bytes of heap
  (refused when cells may not take that much) and replies with the
  identifier of the cell it will be. `load-data <cell> <bytes>` frames
  then carry the file in order; the host sends them all before it reads
  the replies, so the serial line never waits for a round trip. The one
  that completes the file validates it (D4); bytes past the announced size
  end the load, and so does `kill` of its identifier.

A chunk of 64 KiB keeps a frame's reassembly buffer small in the kernel and
lets `rhizo` show progress later; frames of 1 MiB would save nothing
measurable (12 bytes of header per 64 KiB).

### Measured

On the development host (QEMU 8.2.2 with TCG, x86_64 guest on
`qemu-microvm`, 4 CPUs shared with several other QEMU runs, so the
figures move with the load):

| What | Size | `save` (kernel to host) | `load` (host to kernel) |
|---|---|---|---|
| CPython sleeping, suspended (lightly loaded host) | 14.6 MiB | 18.3 s, 0.79 MiB/s | 86 s, 0.17 MiB/s |
| Same, host busy with two in-kernel test runs | 14.6 MiB | 18.6 s, 0.78 MiB/s | 426 s, 0.03 MiB/s |
| Chat agent template after one turn (the benchmark, busy host) | 17.1 MiB | 20.9 s, 0.82 MiB/s | 419 s, 0.04 MiB/s |

The 14.6 MiB file holds 13.2 MiB of pages (6 MiB of them the interpreter
image's three 2 MiB pages, which every CPython cell shares) and 1.4 MiB of
files. On the fresh VM, a cell of the loaded agent template answers its
first turn 0.18 s after the `fork`, against 0.94 s for a new agent from
`run` (`snapshot.loaded_answer_us`, `snapshot.cold_answer_us`): once the
file is in, a downloaded template starts five times faster than a cold
agent.

Sending costs the kernel two port accesses per byte (status, data) and
holds steady. Receiving is bounded by how QEMU feeds the 16550: it hands
the guest at most the FIFO trigger level (14 bytes) per pass of its main
loop, and the next 14 wait for the main loop to wake again, which on a
busy host takes long; waiting between polls in the kernel, so as not to
contend for QEMU's lock, did not help and was dropped. Under KVM each port
access is a VM exit; the order of magnitude stays tens of seconds or more
per agent. That is enough to show the feature end to end and to measure
it, not to start a downloaded agent "instantly".

### Faster path (follow-up)

A virtio-blk **exchange disk**: `rhizo up` creates a file as large as the
VM's memory and attaches it as a virtio-blk device, which QEMU (`microvm`,
`virt`: virtio-mmio; `q35`: virtio-pci), Firecracker (drives) and Cloud
Hypervisor (`--disk`) all offer, on both architectures. `save` and `load`
then name an offset and a length on that disk: the host reads or writes
the file directly and the kernel moves the bytes with a few large DMA
requests (hundreds of MiB/s even under TCG). The kernel side is a block
driver beside `VirtioNet` in `rhizo-virtio` (one request queue: header,
data, status), found like the network card; it is a change of its own:
the driver, its probing on both transports and architectures, the
VMM command lines, and its fuzzing as a new device surface.

Rejected for that follow-up: virtio-vsock (Firecracker and Cloud
Hypervisor expose it as a Unix socket on the host, QEMU needs
`vhost-vsock` in the host kernel: three host sides, and a credit-based
stream protocol in the kernel); TCP over the existing network card
(Firecracker and Cloud Hypervisor have a card only on a tap device, which
needs root; a kernel-side TCP client); a shared memory device
(`ivshmem`, `virtio-pmem`: not offered by every VMM).

Independently, files can shrink: the interpreter's pages and the files of
`cpython-files` are the same in every CPython agent, so a file could name
them by content instead of carrying them, and a kernel that holds them
would send and receive only what the agent made its own (a few MiB).

## D3. Templates in the control plane

The kernel already has templates (`Host::freeze`, `spawn_from_template`).
The control plane gains:

- `freeze`: a running cell becomes `Place::Template` (its output so far
  is kept for its watchers); `list` shows it as `template`.
- `fork` of a template makes cells with `spawn_from_template`, all or
  none, so they keep no time (spec `fork`).
- `kill` of a template drops it (`Host::drop_template`): its tables and
  frames go back once no cell made from it shares them.
- `save` of a template captures it (`Host::template_snapshot`): a
  snapshot that refers to the frames it shares, exported, then dropped.
  Its sockets are recorded with `export_sockets` and unparked at once, so
  the template keeps them and the file holds no parked key.
- `load --template` restores the snapshot eagerly into a cell that never
  enters the scheduler and becomes a template (`Host::load_template`).

A loaded template's threads wait as they waited when frozen (format v9,
D5), so its copies time out their waits on descriptors at once, as copies
of the original did (`fast-node-template`).

## D4. What a file may claim

Loaded bytes are untrusted (boundary B3): they go through `from_bytes`,
checksum first. On top of what it checked before (header, every page
record, every thread and register that would fault in the kernel, the file
system, the socket records), three points matter for files:

- **No physical address.** A record that refers to a frame (`REFERS`, from
  `add-shared-snapshots`) is refused: the mark must be zero, as are the
  other reserved bytes. Only `capture` makes such records, in the kernel.
- **No more quota than a new cell.** A cell's quotas live in its snapshot
  (`add-cell-quotas`); a file could raise its own, notably above the
  default share of sockets (finding S21). `Snapshot::cap_quota` lowers each
  quota to at most `Quota::DEFAULT` (the CPU time used is kept), so a
  loaded cell gets what a cell the host starts gets; the host may lower
  them later.
- **Parked connections.** A record may name a parked connection by boot
  identity and key (S15). In another boot nothing matches and the socket
  comes back reset or bound again; in the boot that saved it, the first
  resume takes the connection back, as for exported bytes before. Keys are
  unpredictable without the boot secret, so a forged file takes nothing.

The fuzzers run on the import path: the tampered-snapshot property test
makes the checksum right again for 9 cases in 10 (as a hostile file has
it), so the edits reach the checks behind it, and leaves it wrong for the
rest; the random-bytes test seals every case.

## D5. Format version 9: waits on descriptors keep their time left

A thread blocked in `poll`, `select` or `epoll_wait` is back at its system
call instruction and makes the call again when it runs; the call's
deadline lives in the host's record of the live cell (`Live::deadlines`),
not in the thread. Version 8 wrote such a thread as runnable without time,
so a resumed cell started the call's timeout over.

Version 9 keeps the thread word layout and gives two meanings to the time
word:

- wait kind 3 (new): waiting on descriptors, with the time its wait had
  left (`u64::MAX`: none). It decodes as `Wait::Io` again.
- wait kind 0 (runnable): the time left of the call on descriptors the
  thread makes again, if it was woken before it could (it was
  `u64::MAX` only).

`capture` takes the cell's kept deadlines; `Snapshot::deadlines(now)`
gives them back as deadlines from `now`. `Host::resume` restores them and
wakes the threads waiting on descriptors, which make their calls again
with the time they had left (the resumed cell joins no wait queue it was
in, so it must look again). A template restored from a file is not woken:
its copies expire every deadline when they start (D3).

Alternative: a separate deadline word per thread. Rejected: the futex
words are unused for these kinds, and a word more per thread grows every
snapshot.

## D6. Benchmarks on QEMU only

`snapshot.save_us`, `snapshot.load_us`, `snapshot.file_bytes`,
`snapshot.loaded_answer_us` and `snapshot.cold_answer_us` are measured by
the control-plane benchmark on the QEMU VMMs, once per run (a save and a
load take tens of seconds over the serial line, D2). Firecracker and Cloud
Hypervisor feed their serial line differently and would add minutes per
VMM to every CI run for a figure the faster transport replaces. The
figures are reported, not gated, until that transport lands: gating a
serial line's rate would gate the VMM, not Rhizo.

`snapshot.loaded_answer_us` (fork of the loaded template to the first
answer read back) next to `snapshot.cold_answer_us` (run of a new agent to
its first answer, on the same fresh VM) is what a downloaded template buys
once it is in the VM; `snapshot.load_us` is what getting it there costs
today.
