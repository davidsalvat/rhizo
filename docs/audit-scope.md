# External security audit: scope

This is the document we would hand to an external security auditor before
an engagement. It describes what Rhizo is, where its trust boundaries are,
what we promise and what we do not, which code is in scope (ranked by
risk, with sizes), what has been done already, and what we would like the
audit to answer. The internal reviews it builds on are in
[`security.md`](security.md); the requirements are in
[`openspec/specs/`](../openspec/specs/).

It describes `main` as of commit `ad7c3ee` (November 2026). Line counts
were taken with `wc -l` on that commit; "code" excludes test modules and
test files, "tests" counts them. Changes merged since then that touch the
scope are noted where they apply: per-cell quotas (`add-cell-quotas`,
closing S20, S21 and O1), KPTI and E0PD (`add-kpti`, closing the
Meltdown part of O3), snapshots that refer to shared frames
(`add-shared-snapshots`), the network review (`fuzz-wire-packets`,
S22, S23), the first cross-cell leaks closed (`close-cross-cell-leaks`,
S24 to S26) and the control channel over virtio-vsock
(`control-over-vsock`, reviewed in
[The control channel over vsock](security.md#the-control-channel-over-vsock-b7-b8):
S27, O8).

## 1. System overview

Rhizo is an operating system kernel written in Rust (edition 2024, stable
toolchain, `no_std`), MIT licensed, for AArch64 (primary) and x86_64. It
runs as a guest under QEMU, Firecracker or Cloud Hypervisor. Its unit of
isolation is the **agent cell**: an address space with threads, files and
sockets that runs unmodified static or dynamically linked Linux programs
(glibc C programs, CPython 3.13) through Linux system calls. Cells can be
created, suspended into a snapshot, resumed, and forked into copies that
share memory copy-on-write.

There are no user/kernel processes in the Unix sense: the kernel hosts
cells directly, and each cell's file system lives in kernel memory, private
to the cell. There is no KASLR and no secrecy about the kernel's layout.

```text
 ┌──────────────────────────── host machine ────────────────────────────┐
 │  rhizo CLI ── Unix socket ── rhizo server                            │
 │                                  │ vsock or serial line (frames)     │
 │  ┌──────────────────────── VMM (trusted) ─────────────────────────┐  │
 │  │  ┌────────────────────── Rhizo kernel ──────────────────────┐  │  │
 │  │  │  control loop (rhizo-control) ◄── vsock, serial line     │  │  │
 │  │  │  cell host, scheduler, SMP, snapshots (rhizo-snapshot)   │  │  │
 │  │  │  system calls (rhizo-abi), files (rhizo-fs)              │  │  │
 │  │  │  sockets (rhizo-net, smoltcp), virtio-net ◄── packets    │  │  │
 │  │  │  page tables, COW (rhizo-mm), user copies (rhizo-cell)   │  │  │
 │  │  └──────────────────────────────────────────────────────────┘  │  │
 │  │    cell A (EL0)    cell B (EL0)    cell C   untrusted          │  │
 │  └────────────────────────────────────────────────────────────────┘  │
 └──────────────────────────────────────────────────────────────────────┘
```

### Trust boundaries

| # | Boundary | Who is on the other side | Trust |
|---|---|---|---|
| B1 | **Cell to kernel**: exceptions, system calls (107 Linux calls decoded, both calling conventions), page faults, timer interrupts | Arbitrary code at EL0 / ring 3 | **Untrusted.** The main boundary. |
| B2 | **Cell to cell**: shared frames after fork and template copies, shared page cache, loopback and Unix sockets, the kernel-wide socket and port pools, and any kernel state one cell changes and another reads (covert channels: the inventory in `add-cell-hierarchy` D12) | Another hostile cell | **Untrusted.** `sysinfo`, ephemeral ports and page-cache eviction no longer cross cells (S24 to S26); what is left is listed in [Cross-cell leaks](security.md#cross-cell-leaks). |
| B3 | **Snapshot bytes to kernel**: `Snapshot::from_bytes` (header, CRC-32C, then everything), `Fs::decode`, socket records; reached from the host by the control kernel's `load` request (`rhizo load`, change `snapshots-on-disk`), the bytes fetched over TCP through the network card from the address the host names, the connection admitted by the host only from the guest's address and with the request's one-time token | Whoever wrote the file: any snapshot file a user loads, such as a downloaded template | **Untrusted.** A loaded file is validated before anything of it is used, refers to no physical frame (`REFERS` must be zero), and its cell gets no more quota than a cell the host starts (`Snapshot::cap_quota`). The checksum only catches damage: a forged file passes it and is contained by validation. Fuzzed on the sealed (import) form. |
| B4 | **Cell files to program loader**: `Host::spawn_file` and `rhizo_abi::exec` start an executable and its `PT_INTERP` interpreter from the cell's own files | A cell that wrote those files | **Untrusted** (ELF headers are hostile). |
| B5 | **Network packets to `smoltcp` and `rhizo-net`**: TCP, UDP, DHCPv4, Ethernet frames from the virtio-net card | Any host on the network the VM is attached to | **Untrusted.** Reviewed and fuzzed (`fuzz-wire-packets`, S22, S23). See note below. |
| B6 | **virtio device to driver**: virtio-mmio and virtio-pci configuration, BARs, virtqueue rings | The VMM's device model | **Trusted**, reviewed for robustness only (a broken device must not crash the kernel). |
| B7 | **Host control protocol to control kernel**: frames (run, suspend, resume, fork, kill, freeze, save, load, write file, input, stats, shutdown) on the connection the host makes to vsock port 1024 of a virtio socket device (`control-over-vsock`: always on Firecracker and Cloud Hypervisor, on QEMU when the host's `/dev/vhost-vsock` is usable), else on the serial line; under the frames, the vsock packets (headers, credit, resets) and the device's rings; a save's or load's bytes on a TCP connection the host admits only from the guest with the request's one-time token (`snapshots-on-disk`) | The host's `rhizo` server, and anyone who can write to the VM's serial line, the server's Unix socket or the VMM's vsock Unix socket (Firecracker, Cloud Hypervisor); on QEMU, any local user, since `AF_VSOCK` connections to a guest need no privilege (O8) | **Trusted host**, but the frame decoder and the vsock stream are treated as an attack surface (never panic, never allocate more than they receive, never send past the peer's credit or hold more than 256 KiB unread). |
| B8 | **Kernel to host**: replies and cell output in frames, on the vsock connection when the host holds one, else on the serial line; console text, on the serial line only | The kernel; cell output travels inside it | Cells are untrusted: their output must never be able to forge a frame the host would act on. |
| B9 | **VMM, firmware, hardware** | The hypervisor and CPU | **Trusted.** Out of scope. |

Note on B5: [`security.md`](security.md) treats the network *device* as
trusted, but the frames it delivers come from the network, which is not.
Since `fuzz-wire-packets` they are a third attacker there, reviewed in
[Frames from the network](security.md#frames-from-the-network-b5) and
fuzzed through `Net::poll_wire` with `smoltcp` as the kernel configures
it (findings S22 and S23). We still ask the audit to look at it (Q11):
the fuzzer is not coverage-guided and runs on the host.

## 2. Threat model

### Attackers

1. **A hostile cell** (primary). Runs any instruction allowed at EL0 /
   ring 3, makes any system call with any arguments, from any number of
   threads on any number of CPUs, and as many cells as the host gives it.
   Knows the source and the memory layout.
2. **A hostile snapshot**. Any byte sequence presented to
   `Snapshot::from_bytes` (future template downloads).
3. **A hostile program image**. ELF executables and interpreters written
   by a cell into its files and then started.
4. **A remote network peer**. Sends arbitrary packets to the VM's card, or
   answers a cell's outbound connection.
5. **A local host user** who can reach the `rhizo` server's Unix socket
   or, on QEMU, the control kernel's vsock port (low priority; see Q15,
   Q17 and O8).

### What we promise

| Property | Statement | Spec |
|---|---|---|
| Kernel integrity | No cell, snapshot, image or packet can read or write kernel memory, change kernel page-table entries, run code at EL1 / ring 0, or make the kernel panic or hang. | [`isolation`](../openspec/specs/isolation/spec.md), [`virtual-memory`](../openspec/specs/virtual-memory/spec.md), [`cpu-exceptions`](../openspec/specs/cpu-exceptions/spec.md) |
| Isolation between cells | A cell reaches only its own memory, threads, registers, files and sockets. Shared frames (fork, templates, page cache, 2 MiB image pages) are never writable by a sharer. A Unix socket is reachable only through a name in the cell's own files. | [`isolation`](../openspec/specs/isolation/spec.md), [`fork`](../openspec/specs/fork/spec.md), [`networking`](../openspec/specs/networking/spec.md), [`multi-core`](../openspec/specs/multi-core/spec.md) |
| No leak across reuse | Every frame given to a cell is zeroed or fully written. Forks, template copies and resumes never share random generator state. A parked TCP connection is re-attached only with its unguessable key. An ended cell's memory is mapped for no one. | [`isolation`](../openspec/specs/isolation/spec.md), [`snapshots`](../openspec/specs/snapshots/spec.md) |
| Hostile input is refused or contained | Snapshot bytes and ELF images are refused, or produce a cell that reaches nothing beyond its own resources. Control frames never panic the decoder and never make it allocate more than it received. | [`snapshots`](../openspec/specs/snapshots/spec.md), [`cells`](../openspec/specs/cells/spec.md), [`control-plane`](../openspec/specs/control-plane/spec.md) |
| Bounded kernel work and memory per call | Range walks skip empty space; names, captured output and console reports are bounded per cell. | [`isolation`](../openspec/specs/isolation/spec.md) |

### Out of scope

- **Denial of service by a cell without quotas.** Per-cell quotas of
  memory, CPU time, descriptors and sockets landed after `ad7c3ee`
  (`add-cell-quotas`, [PR #61](https://github.com/davidsalvat/rhizo/pull/61);
  S20, S21 and O1 fixed): a cell over a quota is refused or ended, not its
  neighbours. Still out of scope: a cell the host gave no memory quota
  taking memory down to the kernel's reserve, and one long call delaying
  others (O2). A *kernel crash or hang* from exhaustion, and getting past
  a quota, are in scope.
- **Side channels** beyond what is already mitigated: caches, branch
  predictors, speculative execution (Spectre, MDS, L1TF), timing, the
  nanosecond clock cells see. Meltdown is mitigated since `add-kpti`
  ([PR #63](https://github.com/davidsalvat/rhizo/pull/63), merged after
  `ad7c3ee`): KPTI on x86_64 CPUs that need it, E0PD on AArch64 where the
  CPU has it (O3). Whether that mitigation is complete is in scope (Q3).
- **VMM, firmware and hardware bugs**, and a malicious device model
  (virtio is reviewed for robustness only).
- **The build and supply chain** beyond `cargo deny` (licenses, advisories,
  sources). The only third-party crate in the kernel is `smoltcp` 0.14
  (0BSD) with its small dependencies (`managed`, `heapless`, `byteorder`,
  `bitflags`, `cfg-if`, `defmt`); see `Cargo.lock`.
- **Host tooling** (`xtask`, the benchmark harness), except the `rhizo`
  server's handling of kernel output (B8) and its Unix socket (question
  Q15).
- **Builds other than the ones cells run in**: the cost accounting and
  sampling features (`profile`, `sampling`) and benchmark-only code
  (`kernel/src/cell.rs` from `pub fn run`, line 2792, on).

## 3. In-scope components, ranked by risk

Risk combines exposure (how directly an attacker reaches it), privilege
(what a bug gives) and how much it has been tested adversarially already.
All library crates below contain **no `unsafe` code**; `unsafe` is
confined to `kernel`, `rhizo-mm` (heap only) and `rhizo-core` (spinlock
only), see section 4.

| Rank | Area | Boundary | Files | Code LOC | Test LOC | Why this rank |
|---|---|---|---|---|---|---|
| 1 | **Kernel entry and exit, privileged state** | B1 | `kernel/src/arch/x86_64/{user,cpu,irq}.rs`, `kernel/src/arch/aarch64/{user,cpu,irq}.rs` | 358 + 608 + 288 + 410 + 270 + 287 = 2,221 | in `kernel/src/tests.rs` | Assembly and system registers on two architectures: `SWAPGS`, `SYSRET`/`IRET` return paths, `SPSR_EL1`/`RFLAGS` sanitising, FP/SIMD lazy switching, SMEP/SMAP, PAN/PXN. Not fuzzed; read and exercised by hostile cells only. |
| 2 | **System-call layer** (`rhizo-abi`) | B1, B2 | `crates/rhizo-abi/src/{lib,files,sockets,poll,threads,pages,exec}.rs` | 1,390 + 656 + 875 + 461 + 228 + 108 + 83 = 3,801 | 2,986 | Every argument of 107 calls; futexes, `clone`/`clone3`, `epoll`, `sendmsg`/`recvmsg`, file `mmap` and the page cache shared by content between cells. Fuzzed on the host with test doubles. |
| 3 | **User memory access and address spaces** (`rhizo-cell`) | B1, B4 | `crates/rhizo-cell/src/{memory,space,elf,threads,lib,images}.rs` | 166 + 287 + 590 + 317 + 492 + 65 = 1,917 | 1,393 | `memory.rs` is the only way into cell memory: it translates in the cell's own table and copies through the direct map, which SMAP/PAN do not guard. The ELF loader takes hostile headers (B4). |
| 4 | **Page tables, copy-on-write, frames** (`rhizo-mm`) | B1, B2 | `crates/rhizo-mm/src/{paging,cow,frames,heap,memmap,lib}.rs` | 690 + 374 + 263 + 185 + 135 + 40 = 1,687 | 1,604 | Two entry formats (x86_64, AArch64), shared page tables between forks, 16-bit frame reference counts (S14), 2 MiB pages. Critical findings S1, S2, S12 were here. Mutation-tested weekly. |
| 5 | **Cell host, SMP, gangs, TLB shootdown** | B1, B2 | `kernel/src/cell.rs` (host part, lines 1 to 2791 of 4,335), `kernel/src/smp.rs`, `kernel/src/arch/{x86_64,aarch64}/smp.rs`, `kernel/src/memory.rs`, `kernel/src/sync.rs`, `crates/rhizo-sched/src/{lib,gangs,idle}.rs`, `crates/rhizo-core/src/sync.rs` | 2,791 + 193 + 346 + 206 + 308 + 79 + 530 + 216 = 4,669 | 668 (sched) + 109 (sync) + in-kernel | Threads of one cell on several CPUs, the cell lock, the x86_64 clean-CPU set and shootdown interrupts, break-before-make on AArch64, freeing a cell from the last CPU to leave it. Races are tested by running them (`racer`), not by a model checker. |
| 6 | **Snapshot decode and restore** | B3 | `crates/rhizo-snapshot/src/lib.rs`, `crates/rhizo-fs/src/codec.rs`, `crates/rhizo-net/src/record.rs`; restore and lazy resume in `kernel/src/cell.rs` (`Lazy`, `Host::resume`, `Host::load_template`); the load path in `kernel/src/control.rs` (`Server::load_data`) | 574 + 535 + 239 = 1,348 (before `snapshots-on-disk`) | 609 (snapshot) | Fully attacker-controlled bytes: `rhizo load` reads them from a file. Validates the checksum, page records, threads, registers, the file tree and socket records; caps quotas. Fuzzed, no finding in D.2. Format version 10. |
| 7 | **Sockets and the TCP/IP stack** (`rhizo-net`, `smoltcp`) | B2, B5 | `crates/rhizo-net/src/{lib,wire}.rs`, `kernel/src/wire.rs`, plus `smoltcp` 0.14 (features `medium-ethernet`, `proto-ipv4`, `socket-tcp`, `socket-udp`, `socket-dhcpv4`) | 1,389 + 548 + 159 = 2,096 (Rhizo) | 2,378 (with the fuzzer) | Shared by all cells: ports, backlogs, buffers, parked connections (S15, S21). Remote packets reviewed and fuzzed since `fuzz-wire-packets` (B5, S22, S23). |
| 8 | **In-memory file system** (`rhizo-fs`, outside the codec) | B1, B2 | `crates/rhizo-fs/src/{lib,io}.rs` | 1,222 + 601 = 1,823 | 962 | Per-cell tree, descriptors, pipes, `epoll`, file blocks shared copy-on-write between forks; limits from S5. |
| 9 | **Control protocol and control kernel** | B7, B8 | `crates/rhizo-control/src/lib.rs`, `kernel/src/control.rs`, UART receive in `kernel/src/arch/{x86_64,aarch64}/console.rs`; the vsock channel `crates/rhizo-virtio/src/vsock.rs` and `kernel/src/vsock.rs` (`control-over-vsock`); host side `cli/src/{serve,client,state}.rs` | 742 + 485 = 1,227 (kernel side, before vsock), plus 501 + 187 = 688 (vsock, tests in their own file); `cli` 1,698 in all | 494 (control) + 274 (cli) + 781 (vsock) | Host is trusted, so the risk is in decoding robustness, output framing (can a cell make kernel console text contain a frame?) and who can reach the channel (O8). Fuzzed by property tests (frames; vsock packets and credit). The vsock layer was reviewed when it landed (S27, O8); the frame decoder and host framing were reviewed by neither D.1 nor D.2. |
| 10 | **virtio drivers** | B6 | `crates/rhizo-virtio/src/{lib,pci}.rs` | 481 + 311 = 792 | 839 | Trusted device; robustness only. S17 found by the configuration-space fuzzer. |
| 11 | **Boot-time parsing** | B9 | `crates/rhizo-core/src/{fdt,acpi,options}.rs`, `crates/rhizo-mm/src/memmap.rs` | 490 + 216 + 261 + 135 = 1,102 | 696 + 107 | Input from the trusted VMM and firmware. Listed for completeness; low priority. |

For orientation, the whole kernel crate is 15,269 lines (12,200 code,
3,069 in its test files), and the library crates hold 29,838 lines (17,205
code, 12,633 tests). Benchmark, profiling and test code inside `kernel/src` is not in
scope.

## 4. `unsafe` code

Counted on `main` with `grep`, excluding comments. Every block must carry a
`// SAFETY:` comment (rule 5 of [`AGENTS.md`](../AGENTS.md); the workspace
denies `unsafe_op_in_unsafe_fn` and warns on `undocumented_unsafe_blocks`
in clippy, which CI runs with `-D warnings`). There are 168 `SAFETY:`
comments in all.

| Crate | `unsafe {}` blocks | `unsafe fn` | `unsafe impl` | `#[unsafe(...)]` | `unsafe extern` | Where |
|---|---|---|---|---|---|---|
| `kernel` | 150 (6 in `tests.rs`) | 10 | 2 | 6 | 9 | Arch code: `aarch64/user.rs` 19, `aarch64/irq.rs` 14, `x86_64/cpu.rs` 12, `x86_64/user.rs` 11, `x86_64/irq.rs` 10, `aarch64/platform.rs` 10, `memory.rs` 8, `cell.rs` 6, the rest 5 or fewer per file. 82 `asm!` sites and `global_asm!` in 7 files (boot, SMP start, entry and exception vectors). |
| `rhizo-mm` | 11 | 3 | 1 | 0 | 0 | `heap.rs` only (kernel heap allocator; run under Miri in CI). |
| `rhizo-core` | 3 | 0 | 1 | 0 | 0 | `sync.rs` only (the spinlock behind kernel globals). |
| `rhizo-abi`, `rhizo-cell`, `rhizo-fs`, `rhizo-net`, `rhizo-snapshot`, `rhizo-virtio`, `rhizo-control`, `rhizo-sched`, `rhizo-hal`, `rhizo-prof` | 0 | 0 | 0 | 0 | 0 | None. Not enforced by `#![forbid(unsafe_code)]` today. |

`cells/` (49 blocks) holds test programs that run *as* cells; they are
attacker code by definition and out of scope.

## 5. What has already been done

### Internal reviews

- **First review (H13, `add-security-review`).** The cell boundary: every
  system call of the time, user copies, address spaces, ELF loading,
  snapshots' registers, resource bounds. Findings S1 to S13.
- **User access guards (`add-user-access-guards`).** SMEP and SMAP on
  x86_64, PAN and PXN on AArch64; found S13.
- **Second review (D.2, `add-security-review-2`).** Several CPUs, sockets
  and the network card (against hostile cells), virtio-pci, dynamic linking
  and file mappings, the image cache and 2 MiB pages, the agent workload's
  host calls, snapshots as untrusted input. Findings S14 to S21.
- **Frames from the network (B5, `fuzz-wire-packets`).** A host on the
  network against `smoltcp` and the wire glue, with a fuzzer. Findings
  S22 and S23.
- **Cross-cell leaks (B2, `close-cross-cell-leaks`).** From the
  covert-channel inventory of `add-cell-hierarchy`: what one cell can
  learn about another through `sysinfo`, ephemeral ports and the page
  cache, with hostile two-cell tests. Findings S24 to S26.
- **The control channel over vsock (B7, B8, `control-over-vsock`).** The
  vsock packet protocol and driver read as an auditor: hostile host-side
  packets, credit overflows, resets, CIDs and ports, the host's
  `/dev/vhost-vsock` policy, and what is and is not fuzzed. Finding S27
  (fixed by `fix-vsock-reconnect`) and open finding O8.

All are written up in [`security.md`](security.md), with the threat
model, a per-call table of checks, each finding's reproduction and fix,
and the tests that keep it fixed. Its per-call table covers every call
the kernel decodes, those added since the first review (threads,
sockets, `poll`/`select`/`epoll`, file `mmap`) included; the second
review's area table says how each area was reviewed.

### Findings

| Status | Findings |
|---|---|
| Fixed, with a regression test | S1, S12 (Critical); S2, S3, S4, S5, S14 (High); S6, S7, S8, S13, S15, S16, S18, S21, S22, S24, S25, S26 (Medium); S9, S10, S11, S17, S19, S20 (Low), S20 and S21 by `add-cell-quotas`; S23 (Low, a `smoltcp` behaviour worked around by `fix-tcp-long-wait`); S24 to S26 by `close-cross-cell-leaks`; S27 (Low) by `fix-vsock-reconnect` (PR #142, in review). Details: [Findings](security.md#findings), [Second review](security.md#second-review-d2), [Frames from the network](security.md#frames-from-the-network-b5), [Cross-cell leaks](security.md#cross-cell-leaks), [The control channel over vsock](security.md#the-control-channel-over-vsock-b7-b8). |
| Open | O1 remainder (a cell without a memory quota; file data outside the memory quota), O2 (call time proportional to what a cell holds), O3 remainder (side channels other than Meltdown, which `add-kpti` closed; an AArch64 CPU with neither E0PD nor CSV3 is reported, not protected), O4 remainder (no hardware entropy: predictable `getrandom`, reported at boot), O5 remainder (no IST stack for NMI or machine check on x86_64), O6 (global cell identifiers), O7 (host-only panics past 65535 copies), O8 (on QEMU any local user can take the control kernel's vsock connection while the host holds none), the cross-cell channels left for `add-cell-hierarchy` (ports as one namespace, page-cache deduplication, shared pools' errors, timing). See [Known limitations](security.md#known-limitations-and-open-findings). |

### Fuzzers (property tests)

Fixed seed (`PROPTEST_RNG_SEED` in `.cargo/config.toml`) on every pull
request through `cargo xtask check`; random seeds and 4096 cases weekly
(`.github/workflows/weekly.yml`). They run on the host against the library
crates with test doubles for physical memory and the TLB, not against the
kernel binary, and they are not coverage-guided.

| Fuzzer | Crate | Input |
|---|---|---|
| `x86_64_cells_cannot_hurt_the_kernel`, `aarch64_cells_cannot_hurt_the_kernel`, `pointers_outside_the_cell_give_efault` | `rhizo-abi` (`src/hostile.rs`) | Up to 40 system calls with any number and arguments |
| `decoded_files_cannot_hurt_the_kernel`, `random_bytes_are_refused_or_work` | `rhizo-abi` | Tampered or random encoded file systems, then calls |
| `tampered_programs_start_safely_or_not_at_all` | `rhizo-abi` (`src/exec.rs`) | Dynamically linked program and interpreter with changed headers |
| `tampered_snapshots_are_refused_or_contained`, `random_bytes_are_refused_or_contained` | `rhizo-snapshot` | Tampered or random snapshots, restored into a cell sharing a kernel half |
| `any_configuration_space_is_set_up_or_refused` | `rhizo-virtio` | virtio-pci configuration space and BAR sizes |
| `hostile_headers_never_panic`, `ranges_outside_the_user_half_are_refused` | `rhizo-cell` | ELF headers; user ranges |
| `any_bytes_never_panic`, `any_payload_never_panics`, `any_input_payload_is_bounded` | `rhizo-control` | Bytes on the control stream, payloads of every kind |
| `hostile_packets_are_survived`, `credit_is_never_exceeded` | `rhizo-virtio` (`src/vsock/tests.rs`) | vsock packets with any addresses, operation, socket type, length and credit between the stream's writes and reads; credit updates with wrapping counters. Not in the weekly random-seed run (its filter `hostile::` misses them) |
| `frames_from_the_wire_cannot_hurt_the_kernel`, `dhcp_replies_cannot_hurt_the_kernel` | `rhizo-net` (`src/hostile.rs`) | Valid, changed, cut, replayed and random Ethernet frames and floods next to two cells' sockets; DHCP replies to a card asking for a lease |

Run them:

```sh
cargo test -p rhizo-abi -- hostile:: exec::
cargo test -p rhizo-snapshot -- hostile::
cargo test -p rhizo-virtio -- hostile:: vsock::
cargo test -p rhizo-cell -- hostile
cargo test -p rhizo-control
cargo test -p rhizo-net -- hostile::
# More cases and a new seed, as the weekly job does:
PROPTEST_RNG_SEED=$RANDOM PROPTEST_CASES=4096 cargo test -p rhizo-snapshot -- hostile::
```

### Hostile cells in the kernel

Programs in `cells/src/bin` run as cells by the in-kernel suite
(`kernel/src/tests.rs`, 117 tests) on both architectures, with 1 and 4
CPUs: `fuzz` (20,000 random calls next to a well-behaved cell),
`hostile_ptrs`, `fault_kernel`, `poke_kernel`, `hog`, `storm`, `spin`,
`scrub`, `namer`, `rewrite`, `flags`, `dice`, `sharer`, `racer`,
`observer` (two cells: one observes `sysinfo`, its ports or a kept page
while the other allocates, binds or exhausts memory). Self-checks
`ktest-user-read` and `ktest-user-exec` show SMAP/PAN and SMEP/PXN fault
on a kernel access to a cell page.

### Other tooling

- **Miri** on the kernel heap allocator: `cargo +nightly miri test -p
  rhizo-mm -- heap::` (CI job `miri`, with
  `MIRIFLAGS=-Zmiri-disable-isolation`).
- **Mutation testing** of `rhizo-mm` and `rhizo-core`, weekly:
  `cargo xtask mutants` (needs `cargo-mutants`).
- **Host tests**: 486 `#[test]` functions plus the property tests above.
- **Dependency policy**: `cargo deny check` (licenses, advisories,
  sources) and `cargo machete` in CI.
- **Differential check**: the cells' expected outputs
  (`cells/expected.rs`) are also checked natively on Linux, so the
  `hostile_ptrs` answers match Linux's.

### Known gaps in what was done

- No coverage-guided fuzzing, and nothing fuzzes the kernel binary itself.
- The entry code, the scheduler loop and the TLB shootdown protocol were
  read and exercised, not fuzzed, model-checked or proven.
- Races are found only when an interleaving happens; CI runs QEMU, with
  software emulation where the runner has no KVM, whose memory ordering is stronger than some
  hardware's.
- The control protocol (B7, B8) has had a review of its vsock transport
  only (S27, O8); the frame decoder and the host's framing have not had a
  dedicated review. The vsock driver is tested against chosen hostile
  device cases, not fuzzed, and the kernel's vsock glue runs only in the
  `control` test step. Remote network input (B5) has had a review, with a
  host fuzzer, but no coverage-guided one.

## 6. Suggested priorities and questions

Ordered by what we most want answered.

**P1. Entry, exit and privileged state (ranks 1, 5).**

- Q1. Can a cell reach EL1 / ring 0 with any register, flag or stack it
  controls? Check `SWAPGS` pairing on every path (including faults during
  entry and exit), `SYSRET` versus `IRET` choices, `SPSR_EL1` and `RFLAGS`
  sanitising, `DF`/`AC` clearing (S13), FP/SIMD state handed between cells.
- Q2. On x86_64, NMI and machine check have no IST stack (O5). Is any
  other exception reachable in the window before the stack switch?
- Q3. Is the KPTI mapping that cells see on x86_64 (`add-kpti`: entry
  pages, per-CPU data, descriptor tables and trap stacks) minimal, and are
  the CR3 switches complete on every entry and exit path?

**P2. User memory and page tables (ranks 3, 4).**

- Q4. `rhizo_cell::memory` translates and then copies through the direct
  map. With threads of the same cell on other CPUs, can a frame be freed
  or reused between the translation and the copy? (The cell lock is meant
  to prevent it.)
- Q5. Can any sequence of `mmap`, `MAP_FIXED`, `mprotect`, `munmap`,
  `brk`, fork and copy-on-write faults leave a shared frame writable, a
  kernel-half entry changed (x86_64 shares the root's upper half), or a
  frame mapped after it was freed? Pay attention to 2 MiB pages and to the
  16-bit reference counts.
- Q6. Are AArch64 break-before-make and TLB invalidation (broadcast) and
  x86_64 PCID handling (clean-CPU set, shootdown interrupts) sufficient on
  real hardware with weaker ordering than QEMU's?

**P3. System calls (rank 2).**

- Q7. Any call whose kernel work or allocation is sized by an argument
  rather than by what the cell holds (beyond O2)?
- Q8. Futexes, `clone3`, `epoll` and blocked calls across suspend, fork
  and `exit_group`: can a wait or a wake reach another cell's thread or a
  freed object?
- Q9. The page cache shares file pages *by content* between cells. Can a
  cell learn whether another cell holds a given page (a deduplication side
  channel), or change a page another cell sees? (Eviction by another cell
  is closed since S26, except of pages no cell maps under memory
  pressure; deduplication is known and left to the hierarchy.) More
  generally, is there kernel state besides those listed in
  [Cross-cell leaks](security.md#cross-cell-leaks) through which one cell
  can signal to another?

**P4. Untrusted input decoders (ranks 6, 7, 9).**

- Q10. Can any snapshot that `from_bytes` accepts resume into a cell that
  reaches beyond its own memory, files and sockets, or takes a parked
  connection that is not its own? Are the unchecked heap and mapping
  cursors really harmless?
- Q11. Can a remote peer crash, hang or exhaust the kernel through
  `smoltcp` or `rhizo-net`'s wire glue (`crates/rhizo-net/src/wire.rs`,
  `kernel/src/wire.rs`), for example with SYN floods, out-of-window
  segments, DHCP replies or oversized frames? Is the `smoltcp` 0.14
  feature set in use free of known issues? (Covered internally since
  `fuzz-wire-packets`: floods were bounded (S22); idle connections (S23)
  were fixed by `fix-tcp-long-wait`; see [Frames from the network](security.md#frames-from-the-network-b5).
  An independent look is still wanted.)
- Q12. Can a cell make the kernel's console text, which shares the serial
  line with the protocol's frames, contain the byte `0x01` and so forge a
  reply or output frame for the host? (Cell output is sent only in frames
  in the control build; kernel messages print numbers only.)
- Q13. Does `rhizo_control::Decoder` hold to "never panics, never
  allocates more than the bytes received" on every path, including frames
  split across reads?

**P5. Robustness and the rest (ranks 8, 10, 11).**

- Q14. Can a misbehaving (not malicious) virtio device make the driver
  read or write outside its rings or BARs?
- Q15. The `rhizo` server's Unix socket (`~/.rhizo/control.sock`, or a
  predictable name in the temporary directory when that path is too long)
  is created with the default umask. Should it be restricted?
- Q16. Should the library crates that have no `unsafe` enforce it with
  `#![forbid(unsafe_code)]`?
- Q17. The control channel over vsock (`crates/rhizo-virtio/src/vsock.rs`,
  `kernel/src/vsock.rs`): can host-side packets or a misbehaving device
  make the stream send past its peer's credit, hold more than it
  advertises, or read one connection's bytes as another's (S27)? Is the
  mitigation proposed for O8 (a secret only the host knows, presented
  first on the connection) sound, and is anything else on the host able
  to reach port 1024?

## 7. How to build and run everything

Requirements are listed in [`AGENTS.md`](../AGENTS.md#build-and-test):
the Rust toolchain via `rustup` (pinned by `rust-toolchain.toml`),
`qemu-system-aarch64` and `qemu-system-x86_64`, gcc with a static glibc
for the C cells, and, for the CPython cell, `zlib1g-dev` and
`RHIZO_CPYTHON_SRC` naming the CPython tarball. KVM is optional for QEMU
and needed for Firecracker and Cloud Hypervisor. Every `cargo xtask`
command takes `--arch aarch64` (default) or `--arch x86_64` right after
`xtask`.

| Command | What it does |
|---|---|
| `cargo xtask check` | Formatting, clippy (all crates, both kernel targets), host tests including the fuzzers at a fixed seed, crate dependency rules. Seconds. |
| `cargo xtask test` | Host tests, boot check, the in-kernel suite with hostile cells, all self-checks. About a minute per architecture. |
| `cargo xtask test --only ktest` | Only the in-kernel suite (1 CPU). |
| `cargo xtask test --only ktest-smp` | The in-kernel suite with 4 vCPUs (races, shootdowns). |
| `cargo xtask test --only ktest-user-read` | Self-check: a kernel read of a cell page must fault (SMAP / PAN). Also `ktest-user-exec`. |
| `cargo xtask --arch x86_64 test --only control` | `rhizo` driving a control kernel over vsock where the VMM has a socket device (on QEMU, when `/dev/vhost-vsock` opens), else over the serial line (the agent part needs the CPython cell). |
| `cargo xtask test --vmm firecracker` | The same on Firecracker (also `cloud-hypervisor`, `qemu-q35`; needs KVM). |
| `cargo xtask run` | Boot a kernel with its console on standard output. `--gdb` waits for a debugger (QEMU). |
| `cargo xtask ci` | Everything CI runs. Minutes. |
| `cargo xtask mutants` | Mutation testing of `rhizo-mm` and `rhizo-core`. Long. |
| `cargo deny check` | Licenses, advisories and sources of dependencies. |
| `rhizo up`, `rhizo run`, `rhizo ps`, `rhizo fork`, `rhizo suspend`, `rhizo resume`, `rhizo kill`, `rhizo down` | The control plane by hand ([`demo.md`](demo.md)). |

Every failure prints `FAILED <kind>` with an exact `reproduce` command.
Hostile test programs are added in `cells/src/bin` and registered in
`kernel/src/tests.rs`; host-side fuzzers live next to the code they test.

## 8. Expected deliverables

1. **A findings report**: each finding with severity (against the threat
   model above), affected architecture(s), affected files, a reproduction
   (a cell program, snapshot bytes, packet capture or host test), and a
   suggested fix.
2. **Proof-of-concept inputs** for every finding rated Medium or above, in
   a form we can turn into regression tests (a `cells/src/bin` program, a
   property-test case, or raw bytes).
3. **Answers to Q1 to Q17**, including "looked, found nothing" with what
   was examined, so we know what coverage the audit had.
4. **Coverage notes**: which files and functions were reviewed in depth,
   which were skimmed, and which were not reached.
5. **Recommendations** for the threat model and the hardening backlog
   (for example KPTI, quotas, `forbid(unsafe_code)`, coverage-guided
   fuzzing targets).
6. **A retest** of our fixes for the findings, within the engagement.

Findings are handled through a private channel until fixed; each fix
follows the project's rule of a test that fails before it and passes
after, and is recorded in [`security.md`](security.md).

## 9. Effort estimate

In auditor-days for an auditor experienced with OS kernels and Rust,
reviewing both architectures. Sizes are from section 3.

| Area | Days | Reasoning |
|---|---|---|
| Ramp-up: architecture, specs, threat model, building and running the suites | 2 | 24 capability specs and two prior reviews to absorb; tooling is one command per step. |
| Entry, exit, privileged state (rank 1) | 5 | About 2,200 lines of dense architecture code with inline and global assembly on two instruction sets; highest privilege; never fuzzed. |
| Cell host, SMP, gangs, TLB shootdown (rank 5) | 5 | About 4,700 lines, much of it concurrent; needs reasoning about interleavings and memory ordering on hardware, which tests cannot show. |
| System-call layer (rank 2) | 5 | About 3,800 lines over 107 calls, but no `unsafe` and already fuzzed; the time goes to threads, futexes, sockets and the shared page cache. |
| User memory, address spaces, ELF (rank 3) | 3 | About 1,900 lines; `memory.rs` is short and central, the ELF loader takes hostile input. |
| Page tables, COW, frames (rank 4) | 3 | About 1,700 lines with subtle invariants in two entry formats; past Critical findings were here, but it is the best tested crate. |
| Snapshot decode and restore (rank 6) | 3 | About 1,350 lines of decoders plus the restore and lazy-resume paths; fully attacker-controlled input. |
| Sockets and remote packets (rank 7) | 4 | About 2,100 lines of Rhizo code plus the parts of `smoltcp` in use; remote input has had one internal review and a host fuzzer (`fuzz-wire-packets`). |
| File system (rank 8) | 1.5 | About 1,800 lines, no `unsafe`, fuzzed through decoded file systems. |
| Control protocol and host framing (rank 9) | 1.5 | About 1,200 lines on the kernel side; trusted peer, so mostly decoder robustness and Q12. |
| virtio and boot-time parsing (ranks 10, 11) | 1 | Trusted inputs; robustness only. |
| `unsafe` sweep across `kernel`, `rhizo-mm`, `rhizo-core` | 2 | 164 blocks, 13 `unsafe fn`, 4 `unsafe impl`: each `SAFETY:` argument checked, beyond those already met in the areas above. |
| Report, proof-of-concept inputs, retest | 3 | Writing up, building reproductions, retesting fixes. |
| **Total** | **39** | About 8 weeks for one auditor, or 4 to 5 weeks for two working in parallel (P1 and P2 to one, P3 and P4 to the other). |

Adjustments:

- Quotas (PR #61) and KPTI (PR #63) have landed since `ad7c3ee`: add
  about 2 days to review them (frame metering on every path; the x86_64
  entry trampoline and CR3 switching).
- [PR #87](https://github.com/davidsalvat/rhizo/pull/87)
  (`add-shared-snapshots`, merged) changed snapshots to refer to shared
  frames instead of copying them: add about 1 day to rank 6.
- A reduced engagement of about 20 days would cover P1, P2 and Q10 to Q12,
  and leave the rest to the internal reviews.

We would freeze one commit for the audit and report any later change to
in-scope files as a diff.

## Notes on the specs

Every merged change is archived, so `openspec/specs/` (the
[`control-plane`](../openspec/specs/control-plane/spec.md) capability
among them) is the current behavior. A change still under review lives in
`openspec/changes/<id>/`.
