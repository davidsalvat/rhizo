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
(`snapshot format version 8; this kernel reads version 10`, `taken on
aarch64; this kernel runs on x86_64`) rather than a checksum mismatch,
while any other damage, a cut file included, reads `checksum mismatch:
the file is damaged or truncated`. The checksum is stripped once checked,
so a decoded snapshot is byte for byte what `capture` writes. In-kernel
snapshots never carry it: `suspend` and `resume` pay nothing.

Page size needs no field: every page record says whether it is 4 KiB or
2 MiB, and both architectures use a 4 KiB granule. A kernel with another
page size, or any other change to what a snapshot means, is a new format
version, which older kernels refuse by its number.

The CRC is computed eight bytes at a time (slicing by 8, 8 KiB of tables
built at compile time), and `rhizo-snapshot` is optimized in test builds
too, as `rhizo-core` is for ChaCha20: in-kernel tests export and decode
snapshots of tens of MiB at every step under emulation, and the first CI
round on AArch64 (TCG) timed out in them with a byte-at-a-time CRC in an
unoptimized build.

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

## D2. Transport: TCP over the network card, requests on the serial line

Requests stay on the serial line; the bytes go over TCP through the VM's
network card, which every control kernel already has (it is how agents
reach their model):

- `save <cell> <host>`: the host opens a TCP listener where the guest
  reaches it (`127.0.0.1` behind QEMU's user network, seen by the guest as
  `10.0.2.2`; the tap's address on Firecracker and Cloud Hypervisor) and
  names it in the request. The kernel exports the snapshot into its heap
  (refused when the heap cannot hold it), connects, sends it, shuts its end
  for writing and replies with the total once the host, having read
  everything, has closed.
- `load <size> <template> <host>`: the host offers the file on such a
  listener. The kernel reserves `size` bytes of heap (refused when cells
  may not take that much), connects, reads exactly that many bytes,
  validates them (D4) and replies with the new cell. A host that closes
  early, or a transfer that moves no byte for 10 s, fails the request;
  nothing of it is kept.

The kernel side is `Host::send_bytes` and `Host::fetch_bytes` in
`kernel/src/cell.rs`: a kernel-owned socket of `rhizo-net`, polled with
the card until the transfer is done. No new device, driver or dependency.
While a transfer runs the control kernel serves nothing else, as during
any request; cells do not run.

A VM without a card with an address (Firecracker and Cloud Hypervisor
when the host cannot make a tap) cannot save or load: the request is
refused with that reason.

### Measured

On the development host (QEMU 8.2.2 with TCG, x86_64 guest on
`qemu-microvm`, 4 CPUs shared with several other QEMU runs, so the
figures move with the load):

| What | Size | Serial (frames of 64 KiB) | TCP through the card |
|---|---|---|---|
| CPython sleeping, suspended: save | 14.6 MiB | 18.3 s (0.79 MiB/s) | 0.49 s (29.5 MiB/s) |
| Same: load | 14.6 MiB | 86 s (0.17 MiB/s) lightly loaded host, 426 s (0.03 MiB/s) busy | 0.15 s (95 MiB/s) |
| Chat agent template after one turn (the benchmark): save | 17.1 MiB | 20.9 s (0.82 MiB/s) | 0.61 s (28 MiB/s) |
| Same: load into a fresh VM | 17.1 MiB | 419 s (0.04 MiB/s), busy host | 0.24 s (71 MiB/s) |

The serial figures come from a first implementation that carried the
bytes in `data` and `load-data` frames on the serial line. Sending cost
the kernel two port accesses per byte; receiving was bounded by how QEMU
feeds the 16550: at most the FIFO trigger level (14 bytes) per pass of its
main loop. Under KVM each port access is a VM exit: on GitHub's runners
the `control` test step's two loads alone ran past the job's 30 minutes.
TCP through the card is 35 to over 500 times faster and needs nothing the
VM does not have.

The 14.6 MiB file holds 13.2 MiB of pages (6 MiB of them the interpreter
image's three 2 MiB pages, which every CPython cell shares) and 1.4 MiB of
files. On the fresh VM, a cell of the loaded agent template answers its
first turn 0.14 s after the `fork`, against 0.83 s for a new agent from
`run` (`snapshot.loaded_answer_us`, `snapshot.cold_answer_us`): a
downloaded template starts six times faster than a cold agent, and its
load takes a quarter of a second.

### Alternatives

- **The serial line** (measured above): no network needed, but tens of
  seconds to minutes per agent.
- **A virtio-blk exchange disk**: `rhizo up` attaches a file as a block
  device (QEMU, Firecracker and Cloud Hypervisor all offer one), and the
  kernel moves the bytes with large DMA requests. As fast or faster, and
  it works without a network, but it needs a new driver, its probing on
  both transports and architectures, VMM command lines and fuzzing as a
  new device surface: a change of its own, worth it if save and load must
  work on a VM without a card.
- **virtio-vsock**: Firecracker and Cloud Hypervisor expose it as a Unix
  socket on the host, QEMU needs `vhost-vsock` in the host kernel: three
  host sides, and a new driver with a credit-based stream protocol.
- **A shared memory device** (`ivshmem`, `virtio-pmem`): not offered by
  every VMM.

Independently, files can shrink: the interpreter's pages and the files of
`cpython-files` are the same in every CPython agent, so a file could name
them by content instead of carrying them (follow-up).

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

A loaded template's threads wait as they waited when frozen (format v10,
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

## D5. Format version 10: waits on descriptors keep their time left

A thread blocked in `poll`, `select` or `epoll_wait` is back at its system
call instruction and makes the call again when it runs; the call's
deadline lives in the host's record of the live cell (`Live::deadlines`),
not in the thread. Versions up to 9 wrote such a thread as runnable without time,
so a resumed cell started the call's timeout over.

Version 10 keeps the thread word layout and gives two meanings to the time
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

## D6. Benchmarks on QEMU, reported

`snapshot.save_us`, `snapshot.load_us`, `snapshot.file_bytes`,
`snapshot.loaded_answer_us` and `snapshot.cold_answer_us` are measured by
the control-plane benchmark on the QEMU VMMs, once per run: the chat agent
starts twice and the VM boots twice, which on every VMM would add a
minute to each CI run. They are reported, not gated, until a few CI runs
give them a baseline (the bench README lists them).

`snapshot.loaded_answer_us` (fork of the loaded template to the first
answer read back) next to `snapshot.cold_answer_us` (run of a new agent to
its first answer, on the same fresh VM) is what a downloaded template buys
once it is in the VM; `snapshot.load_us` is what getting it there costs
today.
