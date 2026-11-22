# Security review of the cell boundary

Rhizo's thesis is that agent cells are isolated *and* fast. This document is
the adversarial half: what a hostile cell is assumed to do, where the trust
boundary is, how every system call checks what crosses it, what the review
found and how each finding was fixed, and what is not covered. It is kept
up to date with the code; the requirements are in
`openspec/specs/isolation` and the tests that keep them true are listed at
the end.

Review of the code as of the `add-security-review` change (milestone H13),
on both architectures (AArch64 and x86_64), extended by a second review
(D.2, change `add-security-review-2`) to everything added since: several
CPUs, sockets and the network card, virtio-pci, dynamically linked
programs and file mappings, the image cache and 2 MiB image pages, the
agent workload's host calls, and snapshots as untrusted input (findings
S14 to S21, section [Second review](#second-review-d2)), then by a review
of frames from the network (boundary B5 of the audit scope, change
`fuzz-wire-packets`: findings S22 and S23, section
[Frames from the network](#frames-from-the-network-b5)), then by the
first pass over the shared state through which cells could observe each
other (change `close-cross-cell-leaks`: findings S24 to S26, section
[Cross-cell leaks](#cross-cell-leaks)), then by a review of the control
channel over virtio-vsock (boundaries B7 and B8 of the audit scope,
change `control-over-vsock`: findings S27 and S28, the latter closed by
`authenticate-control-channel`, section
[The control channel over vsock](#the-control-channel-over-vsock-b7-b8)),
then by a review of colluding cells in different domains against phase 1
of the cell hierarchy (change `add-cell-hierarchy`: findings S29 and S30, section
[Colluding cells](#colluding-cells-add-cell-hierarchy-phase-1)).

The scope we would hand an external auditor (trust boundaries, components
ranked by risk with their size, open questions and an effort estimate) is
in [`audit-scope.md`](audit-scope.md).

## Threat model

**Attacker.** Arbitrary code running in a cell: any instruction the CPU
allows at EL0 / ring 3, any system call number with any six arguments, in
any order, as many times as it likes, from as many cells as the host runs
for it. The attacker knows the kernel's source and memory layout (no
secrecy is assumed: there is no KASLR).

**Goals the kernel must defeat.**

1. Read or write kernel memory (directly, or by making the kernel do it).
2. Read or write another cell's memory, files or registers.
3. Crash, panic or hang the kernel.
4. Use resources without bound in a way that stops other cells (CPU time
   in the kernel, kernel heap, console).
5. Learn another cell's data through recycled frames, a fork, a snapshot
   or a resume.
6. Learn what another cell does, or signal to it, through kernel state
   both can reach without a handle (storage channels), from
   `close-cross-cell-leaks` on; the full inventory and what is left are in
   [Cross-cell leaks](#cross-cell-leaks).

**Second attacker: a hostile snapshot** (from the second review on).
Templates are downloaded as snapshot files (`docs/vision.md`; `rhizo load`
since `snapshots-on-disk`), so the bytes `Snapshot::from_bytes` reads may
be written by anyone: they must be
refused, or resume into a cell that reaches nothing beyond its own memory,
files and sockets, and takes nothing from another cell (memory, files,
parked connections). The same holds for the files a cell writes and the
host later starts as a program (`Host::spawn_file`: the executable and
the interpreter it names come from those files).

**Trusted.** The host code that drives the kernel (`Host::spawn`, `fork`,
`suspend`, `resume`, `kill`, `write_file`): it chooses which executables
run and which snapshots to resume. Executables and snapshot bytes are
still validated. The hardware, firmware and VMM (QEMU, Firecracker, Cloud
Hypervisor) are trusted, and so is the network card they emulate: the
virtio drivers are reviewed for robustness only (a broken device must not
crash the kernel), not against a malicious device. The frames the card
delivers are not trusted: they come from the network (third attacker,
below). The packets of the control kernel's socket device come from the
trusted host, but the vsock stream still parses them as hostile input
(section [The control channel over vsock](#the-control-channel-over-vsock-b7-b8)).

**Third attacker: a host on the network** (from the review of B5 on). Any
host that can reach the VM's network card sends any Ethernet frames, in
any order and as fast as the link allows. It must not crash, hang or
exhaust the kernel, open sockets, change another cell's sockets, or learn
a cell's bytes. It may do what any host may do to its own connections;
and a host that can impersonate a connection's peer (its MAC or IP
address) may reset or redirect that connection, since TCP and ARP have no
authentication on any stack (cells that need more use TLS).

## Trust boundary

```text
 cell code (EL0 / ring 3)            kernel (EL1 / ring 0)
 ─────────────────────────┐ ┌──────────────────────────────────────────────
  instructions ──────────►│ │ exception: fault report, cell killed
                          │ │   (or a lazy page / copy-on-write resolved)
  SVC / SYSCALL ─────────►│ │ Abi::handle (crates/rhizo-abi)
    number + 6 registers  │ │   ├─ user memory: rhizo_cell::memory::read/write
                          │ │   │    range < USER_END, the cell's own table,
                          │ │   │    USER (+ writable) pages, copied through
                          │ │   │    the direct map
                          │ │   ├─ address space: rhizo_cell::space
                          │ │   │    (lower half only, W^X, reserve kept)
                          │ │   └─ files: the cell's own rhizo_fs::Fs
  timer interrupt ───────►│ │ preemption (rhizo-sched)
 ─────────────────────────┘ └──────────────────────────────────────────────
```

What makes the boundary hold:

- **Privilege.** Cell code runs at EL0 / ring 3. On entry the kernel sets
  only the condition flags of the saved state (AArch64 `SPSR_EL1` keeps
  NZCV; x86_64 `RFLAGS` keeps CF, PF, AF, ZF, SF, DF, OF and sets IF), so
  a cell cannot pick its privilege level or mask interrupts.
- **Address spaces.** Each cell has its own page-table root for the lower
  half. On AArch64 the kernel lives in `TTBR1_EL1` and is never in a
  cell's table, and `TCR_EL1.E0PD1` (where the CPU has FEAT_E0PD) makes
  cell accesses to it fault in constant time; on x86_64 the cell's root
  shares the kernel's upper-half entries, all without the user bit, or,
  with KPTI (see *Speculation*), only the entry pages. Address-space tags
  (ASID / PCID) are flushed when a tag changes hands. User pages are never
  executable by the kernel on AArch64 (`PXN`).
- **Copies.** The kernel never dereferences a cell address. Every copy
  translates the address in the cell's own table, requires the user bit
  (and writability for writes) on the effective permissions, and copies
  through the kernel's direct map. A range that reaches past `USER_END`
  (`0x7fff_ffff_f000`) is refused before any translation (finding S1).
- **Guards on user pages** (`add-user-access-guards`). The CPU enforces
  the rule above: x86_64 runs with SMEP and SMAP, AArch64 with PAN (set
  again by every exception, `SCTLR_EL1.SPAN` clear) and `PXN` on every
  cell page, so a kernel load, store or jump at a cell's address faults.
  No `stac`/`clac` or PAN toggle exists anywhere: the only way into cell
  memory, `rhizo_cell::memory`, goes through the direct map, which these
  guards do not cover (its pages are supervisor pages). The direct map is
  guarded by that module's software checks alone. Self-checks
  `ktest-user-read` and `ktest-user-exec` show the guards at work.
- **Flags.** A cell's `RFLAGS.DF` and `RFLAGS.AC` are cleared on every
  entry to the kernel: `SFMASK` for `SYSCALL`, `push 2; popfq` first
  thing for exceptions and interrupts from ring 3 (finding S13).
- **Copy-on-write.** Shared tables and pages are read-only for every
  sharer, even after `mprotect` asks for writes (finding S12); a write
  fault copies only what is shared, and only in the lower half (S2).
- **Per-cell state.** Registers, FP/SIMD state (loaded as a thread that
  uses it is switched in, trapped on first use otherwise, never left in
  the registers for another thread: see *Speculation*), files and
  descriptors (`rhizo_fs::Fs`, private copy-on-write) and the system-call
  handler (`Abi`) belong to one cell. On x86_64 CPUs whose `FXRSTOR`
  keeps the previous x87 instruction and data pointers (AMD and Hygon
  without `XSAVEERPTR`, and QEMU's emulation), an x87 load of a kernel
  variable overwrites them before each FP/SIMD load
  (`scrub-x87-pointers`).
- **Per-CPU kernel data** (`add-smp-boot`). The entry code finds the
  kernel stack and the running cell's registers through per-CPU data that
  cell code cannot redirect: `TPIDR_EL1` on AArch64 (not accessible at
  EL0), and on x86_64 the kernel GS base, which is swapped out with
  `SWAPGS` before every return to a cell and back in at every entry, so a
  cell loading GS or changing its own GS base only changes its copy.
- **TLB coherence across CPUs** (`add-smp-scheduling`). A cell runs on one
  CPU at a time. AArch64 invalidations of cell translations are broadcast.
  On x86_64 each cell records the CPUs whose TLB holds nothing stale for
  it; any other CPU drops the cell's PCID before it next runs the cell, so
  a translation removed by `munmap`, `mprotect`, copy-on-write or a fork,
  or left by a previous holder of the tag, is never used. A CPU lets go of
  a cell's tables when it gives the cell back, so tables are never freed
  while a CPU still has them loaded.
- **Threads of a cell on several CPUs** (`add-parallel-threads`). While a
  cell's threads run on several CPUs, its system calls and the faults that
  change its tables take its lock, one at a time. On x86_64 a call or
  fault that shrinks its translations first interrupts the other CPUs
  running its code and keeps them out until it is complete (they drop the
  PCID when they enter again), so a frame it frees is never reachable
  through another CPU's TLB; AArch64 invalidations stay broadcast. The
  cell is freed only by the last CPU to leave it, after `exit_group` or a
  fault interrupted the others.
- **Zeroed memory.** Every frame a cell is given is zeroed or fully
  written first: anonymous mappings and the heap (`map_anonymous`), ELF
  pages and the stack (`zero_frame` before copying), page tables, private
  copies of shared pages (whole-page copy), resumed pages (whole-page
  copy), new file blocks (`[0; 4096]`).

## Quotas

`add-cell-quotas` closed O1. The host sets a cell's quotas with
`Host::set_quota`; without one a cell has the kernel's limits, as before,
and at most 256 sockets (S21).

- **Memory** (bytes of pages and page tables): every frame the kernel takes
  for the cell goes through `rhizo_cell::quota::Metered`: system calls,
  lazy pages, stack growth, holes and pages past the end of mapped files
  (S20), spawn and resume. Copy-on-write copies are not metered: each
  replaces a shared frame the cell gives back, net zero. Past
  the quota, `mmap` and `brk` answer `ENOMEM`; a fault that needs a frame
  kills the cell, as when memory runs out. Frames given back are credited
  (never below zero), so a forked cell holds at most its quota plus what it
  inherited (counted to its parent). File data (`rhizo_fs::DATA_MAX`) and
  names (4096) keep their own bounds per cell, outside the memory quota: a
  cell may hold that much file data above its quota.
- **CPU time**, its system calls included, counted per slice from when
  the quota is set (a cell without one is not timed); past the quota the cell ends (`Ending::OutOfCpu`,
  `cell N killed: CPU time quota used up`) at the end of that slice, at
  most 10 ms and one call late.
- **Descriptors** (at most 1024): `EMFILE` past the quota. **Sockets**:
  `ENFILE` past the quota, 256 by default (S21).
- **Cell state.** Quotas live in the `Cell`: forks and template copies keep
  them (with no usage), snapshots (format version 8 on) keep them and the
  CPU time used, so a suspend and resume buys no time. `getrlimit` reports
  them; nothing in a cell can raise them, and a snapshot file cannot
  either: a loaded one gets at most a new cell's (`snapshots-on-disk`).

## Speculation (Meltdown, LazyFP)

`add-kpti` closed the Meltdown part of O3, `lazy-fp-switch` LazyFP.

- **x86_64: KPTI.** On CPUs open to rogue data cache load (Intel CPUs
  without `IA32_ARCH_CAPABILITIES.RDCL_NO`, unknown vendors; not AMD or
  Hygon), the kernel half of a cell's tables maps only the entry pages:
  the entry code (two page-aligned sections), per-CPU data, GDTs, TSSs,
  IDT and trap and double-fault stacks, supervisor only. Nothing of the
  direct map, heap, kernel data, code or page tables is mapped while a
  cell runs. The entry code switches to the kernel's tables first thing
  on every system call, exception or interrupt from a cell, and back last
  thing before `SYSRETQ`/`IRETQ` (the frame for `IRETQ` is built on the
  mapped trap stack). The kernel keeps PCID 0 and each cell its own, so
  neither switch flushes the TLB; an untagged cell's CR3 always flushes
  PCID 0. Kernel flushes of a cell's translations cannot reach its PCID,
  so the cell flushes it on its next entry on every CPU. The test build
  runs every in-kernel test with KPTI on.
- **AArch64: E0PD.** Where the CPU has FEAT_E0PD, `TCR_EL1.E0PD1` is set:
  what KPTI buys, for free. A CPU with neither E0PD nor `CSV3` (Cortex-A75)
  is reported at start (`may be open to Meltdown`); there is no trampoline.
- **Cost.** Every benchmark run measures `cell.syscall_ns` and, with KPTI
  forced on, `cell.syscall_kpti_ns`, `cell.switch_kpti_ns` and
  `cell.run_kpti_us` (`bench/README.md`), so the price is known even on
  runners whose CPUs do not need it. On CI's KVM runners (nested
  virtualization, PCIDs): a system call 70 → 187 ns, a switch between
  cells 234 → 281 ns (+20 %), spawn to exit of `hello` 7.0 → 7.2 µs.
- **FP/SIMD state (LazyFP, CVE-2018-3665)** (`lazy-fp-switch`). Trapping
  FP/SIMD while another thread's values stay in the registers would let
  an FP/SIMD instruction that runs speculatively before the trap read
  them. The registers never hold another thread's or cell's values while
  cell code runs: a thread's state is saved when it stops running on a
  CPU, and the next thread either has its own loaded (it has used
  FP/SIMD) or finds the registers cleared to the initial state, with
  FP/SIMD trapping until its first use. Both architectures do the same,
  whether or not the CPU is affected. The in-kernel test
  `fp_registers_hold_nothing_of_another_cell` reads the registers after
  a cell that never used FP/SIMD ran right after one that filled them.
- **Left open.** Spectre v1/v2, L1TF, MDS and their mitigations; the entry
  pages themselves are readable by speculation (they hold per-CPU stack
  pointers and the cell's own saved RDI, no other cell's data).

## Randomness

`add-crypto-random` closed O4. `getrandom` and `AT_RANDOM` come from
ChaCha20 (RFC 8439, `rhizo_core::random`, checked against the RFC's
vectors) run with fast key erasure: every request first replaces the key,
so the kernel's memory never holds what would reproduce bytes already
handed out.

- **Seed.** The kernel's generator is seeded at start from the CPU,
  through `rhizo_hal::Entropy`: `RDSEED`, else `RDRAND`, on x86_64;
  `RNDR` on AArch64. Without one, the seed is the cycle counter and the
  console says `getrandom is predictable`.
- **One generator per cell.** A cell's generator is split off the
  kernel's (keyed with 32 of its bytes) the first time the cell asks.
  `AT_RANDOM` comes from the kernel's generator at spawn.
- **Forks, templates, snapshots.** No generator state is copied into a
  child or written into a snapshot (`Generator` is not even `Clone`): each
  child, template copy and resume splits a new one off the kernel's when it
  first asks, so none draws what another draws, and a snapshot resumed
  twice does not replay. What a cell keeps in its own memory (a libc's
  buffered bytes, its stack-protector value from `AT_RANDOM`) is copied
  like the rest of its memory: a program that forks itself into templates
  should draw fresh bytes after the fork, as on Linux.
- **Planned: copies told to reseed** (`add-cell-hierarchy`, design D22).
  Generators in user space (CPython's `random`, Node.js's `Math.random`,
  OpenSSL's pool) are copied with memory, so two copies of one template
  draw the same sequence today. The design gives each cell a copy
  generation (`/run/rhizo/generation`), honours `MADV_WIPEONFORK` across
  every kind of copy, gives a copy a `getpid` unlike its source's, makes
  the shipped templates reseed, and refuses to publish a template that
  may hold secrets. Copies of a template keep sharing its string hash
  seed, stack-protector value, pointer guard and address layout, as any
  fork server's children do.

## System calls

Every call is decoded from the saved registers by `Arch::decode`; numbers
that decode to nothing return `-ENOSYS`. General rules that every row
relies on:

- **Pointers** go through `memory::read` / `memory::write` (above): a bad
  pointer is `-EFAULT`; calls that stream (`read`, `write`, `readv`,
  `writev`, `getrandom`) return the bytes done before the fault, as Linux does.
- **Descriptors** are the low 32 bits of the argument (Linux's `int`), looked
  up in the cell's own table (`-EBADF` otherwise); at most 1024.
- **Lengths** are never used to size a kernel buffer, except where capped
  (paths: 4096 bytes; `getdents64`: 64 KiB; `poll` arrays and `select`
  sets: 1024 descriptors; a pipe write: 64 KiB; a socket send: one
  datagram or the 256 KiB socket buffer). Data moves in pieces of at
  most one page (and one file block, or 256 bytes for the console), so a
  huge length costs time only for memory the cell really has.
- **Paths** are read up to their NUL, at most `PATH_MAX` (4096) bytes,
  never past the page holding the NUL.

| Call (AArch64 / x86_64 number) | Takes from the cell | Checked |
|---|---|---|
| `read` (63/0), `pread64` (67/17), `readv` (65/19) | fd, buffer, length, offset, iovec array | a file one piece at a time into the buffer, `-EFAULT` before any byte; offset ≥ 0 and only on files (`ESPIPE`); pipes, event counters and sockets as below; console: 0 bytes; `readv` count ≤ 1024 (`EINVAL`), each 16-byte entry copied in, then as `read` |
| `write` (64/1), `pwrite64` (68/18), `writev` (66/20) | fd, buffer, length, offset, iovec array | console in 256-byte pieces; files one piece at a time, each asking memory (`can_store`) first, ≤ 256 MiB of file data per cell (`ENOSPC`/`EFBIG`); pipes, event counters and sockets as below; `writev` as `readv` |
| `exit`, `exit_group` (93, 94 / 60, 231) | status | low 8 bits kept; `exit` of one thread among several ends only it: its clear-tid word is zeroed (if mapped) and one waiter woken |
| `clone` (220/56), `clone3` (435/435) | flags, stack, thread pointer, tid addresses; `struct clone_args` and its size | threads only: without `CLONE_VM`, `CLONE_SIGHAND` and `CLONE_THREAD`, `-ENOSYS` (cells have no processes); unknown flags `EINVAL`; thread pointer below the lower half's end (`EPERM`); the thread's record asked from memory first (`EAGAIN`); `clone3` size 64–4096 bytes, at most 88 read, no exit signal, `set_tid` or cgroup (`EINVAL`); tid words written as on Linux (a bad address does not undo the thread) |
| `futex` (98/202) | address, operation, value, timeout, bitset | `WAIT`, `WAKE` and their `_BITSET` forms only (`ENOSYS`), `FUTEX_CLOCK_REALTIME` only with `WAIT_BITSET`; address 4-byte aligned and bitset non-zero (`EINVAL`); a wait reads the 4-byte word through `memory::read` (`EFAULT`), `EAGAIN` if it changed; timeout read as for `nanosleep`; a wake touches no memory and wakes only the cell's own waiters |
| `brk` (214/12) | new break | between the heap start and `MMAP_BASE`; growth asks memory first (finding S11), zeroed, `rw-`, never executable |
| `mmap` (222/9) | address, length, protection, flags, fd, offset | private only (`EINVAL`; shared file mappings `ENODEV`); writable and executable: anonymous only (files `EACCES`), a write-or-execute range without memory, at most 64 ranges (`ENOMEM`, `node-jit`); length rounded with overflow check; files: a readable regular file (`EBADF`, `EACCES`, `ENODEV`), offset page aligned, its pages shared from the page cache copy-on-write; `MAP_FIXED`: page aligned, end ≤ `USER_END` (`EINVAL`), not page 0 (`EPERM`), replaces what was there; otherwise the kernel picks the address below the stack's floor (`ENOMEM`); `PROT_NONE` reserves addresses without memory; anonymous memory asked first (S11) |
| `munmap` (215/11) | address, length | page aligned, length > 0, end ≤ `USER_END` (`EINVAL`); holes skipped by table entry (S4); 2 MiB pages not split |
| `mprotect` (226/10) | address, length, protection | aligned, known bits only; end ≤ `USER_END` (`ENOMEM`, S1); writable and executable: every page checked first, mapped pages become write-or-execute pages (writing), holes must be reserved and join the ranges, skipped one empty table entry at a time; any other protection takes its part out of the ranges first (at most 64, `ENOMEM`, `node-jit`); `PROT_NONE` frees the pages; otherwise every page mapped, brought in, or reserved and backed now, else `ENOMEM` at the first one (S4); part of a 2 MiB page `EINVAL`; a shared frame becomes writable only copy-on-write (S12) |
| `madvise` (233/28), `sigaltstack` (132/131) | — | accepted, nothing read or changed |
| `getpid`, `gettid`, `set_tid_address` (172, 178, 96 / 39, 186, 218) | address | the cell's or the thread's identifier; the address is kept as the thread's clear-tid word |
| `getuid` … `getegid` (174–177 / 102, 104, 107, 108), `getppid` (173/110) | — | 0 |
| `uname` (160/63) | buffer | 390 bytes written |
| `clock_gettime` (113/228), `clock_getres` (114/229) | clock, buffer | known clocks only (`EINVAL`); 16 bytes written |
| `gettimeofday` (169/96), `time` (—/201) | buffers | 16 (and 8 for the time zone) and 8 bytes written, each unless null |
| `getrandom` (278/318) | buffer, length | 256-byte pieces until the length or a fault; from the cell's own ChaCha20 generator (see *Randomness*) |
| `rt_sigaction` (134/13), `rt_sigprocmask` (135/14) | old-state buffer, size | new state not read; at most 32 / 128 zero bytes written |
| `ioctl` (29/16) | fd, request, argument | `FIONBIO` (4 bytes read), `FIOCLEX` and `FIONCLEX` on the cell's own descriptors; anything else `-ENOTTY` |
| `arch_prctl` (—/158) | code, address | `ARCH_SET_FS` only below `USER_END` (`EPERM`, S3); `ARCH_GET_FS` writes 8 bytes; others `EINVAL` |
| `prctl` (167/157), `capget` (90/125) | option, buffer; header, data | `PR_SET_NAME` accepted (not kept), `PR_GET_NAME` writes 16 zero bytes, others `EINVAL`; `capget` reads the 8-byte header and writes empty sets (12 or 24 bytes) |
| `sched_yield` (124/24) | — | the cell goes to the back of the run queue |
| `sched_getaffinity` (123/204), `sched_getparam` (121/143), `sched_getscheduler` (120/145) | size, buffer, pid | affinity size ≥ 8 (`EINVAL`), 8 bytes written; pid 0, the cell or one of its threads (`ESRCH`); normal policy, priority 0 |
| `nanosleep` (101/35), `clock_nanosleep` (115/230) | clock, flags, `timespec` | 16 bytes read; negative or ≥ 10⁹ ns `EINVAL`; known clocks only; deadline saturates; only this thread waits |
| `pipe2` (59/293), `pipe` (—/22) | flags, 8-byte buffer | flags `O_NONBLOCK` and `O_CLOEXEC` only (`EINVAL`); both ends closed again if the buffer faults; a write asks memory first (`ENOBUFS`), at most 64 KiB held |
| `eventfd2` (19/290), `eventfd` (—/284) | initial count, flags | flags `EFD_SEMAPHORE`, `O_NONBLOCK` and `O_CLOEXEC` only; reads and writes of exactly 8 bytes (`EINVAL`) |
| `poll` (—/7), `ppoll` (73/271) | array, count, timeout | count ≤ 1024 (`EINVAL`); the array copied in and out whole; unknown descriptors `POLLNVAL`; timeout read as above; the signal mask ignored |
| `select` (—/23), `pselect6` (72/270) | descriptor count, three sets, timeout | count 0–1024 (`EINVAL`); sets read and written as count/64 words, an unknown descriptor `EBADF`; without descriptors a sleep, whose null timeout is `EINVAL` |
| `epoll_create1` (20/291), `epoll_create` (—/213), `epoll_ctl` (21/233), `epoll_wait` (—/232), `epoll_pwait` (22/281) | flags, size, operation, fd, event, buffer, count, timeout | flags `EPOLL_CLOEXEC` only, size > 0; operations add, delete and modify (`EINVAL`), the event in each architecture's layout (12 or 16 bytes); count > 0 (`EINVAL`), at most one event per registered descriptor written |
| `socket` (198/41), `socketpair` (199/53) | family, type, protocol, 8-byte buffer | `AF_UNIX` stream, `AF_INET` stream and datagram (`EAFNOSUPPORT`, `ESOCKTNOSUPPORT`, `EPROTONOSUPPORT`); pairs `AF_UNIX` stream only; the descriptors released again if the buffer faults |
| `bind` (200/49), `connect` (203/42) | fd, address, length | a socket descriptor; address length 2–128 bytes read (`EINVAL`), `AF_INET` ≥ 16 bytes and of the socket's family (`EAFNOSUPPORT`), Unix paths ≤ 108 bytes, no abstract names; Unix names in the cell's own file system (`EADDRINUSE`); ports from the kernel's shared table; a blocking connect through the card waits |
| `listen` (201/50), `accept` (202/43), `accept4` (242/288), `shutdown` (210/48) | fd, backlog, address buffer and length, flags, how | backlog ≤ 4096; `accept4` flags `SOCK_NONBLOCK` and `SOCK_CLOEXEC` only; stream sockets only (`EOPNOTSUPP`); the peer's address written up to the room given, the new descriptor closed if that faults; `how` 0–2 |
| `sendto` (206/44), `sendmsg` (211/46) | fd, buffer or `msghdr`, flags, destination | datagrams ≤ 65,507 bytes (`EMSGSIZE`), stream sends cut to the 256 KiB socket buffer; `msghdr` 56 bytes, ≤ 1024 iovec entries (`EMSGSIZE`), each length ≤ `SSIZE_MAX`, their sum saturating (S16); memory asked first (`ENOBUFS`); no control data |
| `recvfrom` (207/45), `recvmsg` (212/47) | fd, buffer or `msghdr`, flags, address buffer | written piece by piece into the cell's buffers; `MSG_PEEK`, `MSG_TRUNC` and `MSG_DONTWAIT`; the source address written up to the room given; no control data, `msg_flags` written |
| `getsockname` (204/51), `getpeername` (205/52) | fd, buffer, length | the address written up to the room given (negative room `EINVAL`), its full length after it |
| `setsockopt` (208/54), `getsockopt` (209/55) | fd, level, name, value, length | known options only (`ENOPROTOOPT`; TCP options on TCP sockets only, `EOPNOTSUPP`); value length checked (`EINVAL`), at most 16 bytes read; ranges checked |
| `set_robust_list` (99/273) | — | accepted, never walked |
| `rseq` (293/334), `io_uring_setup` (425/425) | — | `-ENOSYS` (declined, not reported) |
| `prlimit64` (261/302), `getrlimit` (163/97) | resource, old-limit buffer | resource ≤ 15 (`EINVAL`); new limit and pid not read; 16 bytes written; the cell's quotas for `RLIMIT_CPU`, `RLIMIT_AS`, `RLIMIT_NOFILE` |
| `sysinfo` (179/99) | buffer | 112 bytes written |
| `readlink` (—/89), `readlinkat` (78/267) | — | `-ENOENT` (no links exist), nothing read |
| `fstat` (80/5), `statx` (291/332) | fd, buffer; dirfd, path, flags, mask, buffer | 128 / 144 bytes written; `statx` 256 bytes, its path as `newfstatat` |
| `newfstatat` (79/262), `stat` (—/4), `lstat` (—/6) | dirfd, path, buffer, flags | dirfd as `int` (`AT_FDCWD`, else ≥ 0 or `EBADF`); path; `AT_EMPTY_PATH` |
| `openat` (56/257), `open` (—/2) | dirfd, path, flags | access mode 3 `EINVAL`; with `O_CREAT`, memory asked first and ≤ 4096 files and directories per cell (`ENOSPC`, S5); ≤ 1024 descriptors (`EMFILE`) |
| `close` (57/3) | fd | own table; a socket is released with its last descriptor |
| `lseek` (62/8) | fd, offset, whence | whence 0–2; result checked for overflow and ≥ 0 |
| `getdents64` (61/217) | fd, buffer, size | a directory fd; at most min(size, 64 KiB) built (S5) |
| `mkdirat` (34/258), `mkdir` (—/83) | dirfd, path | memory asked first, file limit (S5) |
| `unlinkat` (35/263), `unlink` (—/87), `rmdir` (—/84) | dirfd, path, flags | only `AT_REMOVEDIR`; non-empty directories refused |
| `renameat` (38/264), `renameat2` (276/316), `rename` (—/82) | two dirfds and paths, flags | flags must be 0; a directory cannot move into itself |
| `getcwd` (17/79) | buffer, size | `ERANGE` if too small; the path is bounded by the file limit |
| `chdir` (49/80) | path | must be a directory |
| `ftruncate` (46/77) | fd, length | a file; 0 ≤ length (`EINVAL`) ≤ 256 MiB (`EFBIG`) |
| `fsync`, `fdatasync` (82, 83 / 74, 75), `fadvise64` (223/221) | fd | checked, nothing else (`fadvise64` on a pipe or socket: `ESPIPE`) |
| `dup` (23/32), `dup2` (—/33), `dup3` (24/292), `fcntl` (25/72) | fds, flags, command | target < 1024; `dup3` flags only `O_CLOEXEC`; known commands only |
| `faccessat` (48/269), `faccessat2` (439/439), `access` (—/21) | dirfd, path, mode | mode bits ≤ 7 |
| anything else | number | `-ENOSYS`; one console report per number below 1023 and one for all larger (S9) |

## Findings

Each finding says how it was confirmed before its fix: a host test that
failed, an in-kernel test that hung or panicked, a reproduction in the
kernel under QEMU, or (for the low ones) reading the code. Severity is for
the threat model above.

| # | Severity | Arch | Finding | Status |
|---|---|---|---|---|
| S1 | Critical | x86_64 | `mprotect` accepted kernel addresses and rewrote kernel page-table entries with the user bit: a cell could map all of RAM for itself | Fixed |
| S12 | Critical | both | `mprotect` made a read-only page shared with a fork (code, constants, a template's pages) writable in place: one cell could write another's code | Fixed |
| S2 | High | x86_64 | A write to a kernel address was taken for a copy-on-write fault: the cell was never killed, and kernel table entries could be changed | Fixed |
| S3 | High | x86_64 | `arch_prctl(ARCH_SET_FS)` accepted any base; loading a non-canonical one faults in the kernel | Fixed |
| S4 | High | both | `munmap` / `mprotect` took one step per 4 KiB page of their range, even unmapped: one call could stop every cell for hours | Fixed |
| S5 | High | both | File and directory names grew the kernel heap without limit or memory check: the heap could fail to grow, which panics | Fixed |
| S6 | Medium | both | Captured output grew without limit (same panic) | Fixed |
| S7 | Medium | x86_64 | An executable could map code in the last page of the lower half, where a `syscall` returns to a non-canonical address | Fixed |
| S8 | Medium | both | Snapshot bytes with a kernel program counter (or, on x86_64, thread pointer or reserved `MXCSR` bits) were accepted and would fault in the kernel on resume | Fixed |
| S9 | Low | both | Unknown call numbers ≥ 1024 were reported on the console every time | Fixed |
| S10 | Low | both | ELF program-header arithmetic could overflow (a panic in debug builds) | Fixed |
| S11 | Low | both | `mmap` / `brk` beyond free memory zeroed frames until running out before refusing | Fixed |
| S13 | Medium | x86_64 | An exception or interrupt from ring 3 kept the cell's direction and alignment-check flags in the kernel (Rust code ran with DF set; AC would lift SMAP) | Fixed |
| S14 | High | both | Mapping one file page 65536 times overflowed the frame's 16-bit reference count: a cell panicked the kernel | Fixed (`fix-shared-frame-count`) |
| S15 | Medium | both | Parked sockets' keys counted up from 1: a forged snapshot could take another suspended cell's TCP connection | Fixed (`fix-parked-keys`) |
| S16 | Medium | both | `sendmsg` / `recvmsg` summed buffer lengths with overflow: a cell panicked a kernel built with overflow checks (debug, the tests' build) | Fixed |
| S18 | Medium | x86_64 | An executable's or interpreter's entry point past `USER_END` was accepted; entering the cell there faults in the kernel | Fixed |
| S17 | Low | both | virtio-pci accepted BAR size masks that are not a power of two, or that wrap past the top of memory: a broken device could have the kernel map memory outside its BAR | Fixed |
| S19 | Low | both | `dup2(fd, fd)` returned its 64-bit argument as given, not the descriptor (neither a value nor an `errno` for a large one) | Fixed |
| S20 | Low | both | `mmap` of a file maps holes and pages past its end with fresh zeroed frames without asking memory first (as S11 did for anonymous memory) | Fixed (`add-cell-quotas`) |
| S21 | Medium | both | Sockets are bounded kernel-wide (4096): one cell can hold about half, two all, and every other cell gets `ENFILE` | Fixed (`add-cell-quotas`) |
| S22 | Medium | both | A flood of frames from the network kept the CPU that polls the card in `smoltcp` for as long as it lasted | Fixed (`fuzz-wire-packets`) |
| S23 | Low | both | A connection to the outside on which nothing arrived for 20 s was reset (`smoltcp` 0.14 applies its timeout to idle connections) | Worked around (`fix-tcp-long-wait`) |
| S24 | Medium | both | `sysinfo` reported the kernel's free memory: every cell saw every other cell's allocations, a storage channel between cells | Fixed (`close-cross-cell-leaks`) |
| S25 | Medium | both | Ephemeral ports came from one counter for all cells: the distance between a cell's ports counted every other cell's binds | Fixed (`close-cross-cell-leaks`) |
| S26 | Medium | both | Any cell could empty the whole page cache (65,536 pages of its own files, or memory pressure from a cell without a quota): the flush half of a flush-and-probe timing channel; the frames of mapped pages then left the cache uncounted in their mappers' quotas | Fixed (`close-cross-cell-leaks`) |
| S27 | Low | both | A vsock connection that replaced the previous one before the control kernel looked got no hello, and its first bytes completed the old connection's unfinished frame | Fixed (`fix-vsock-reconnect`, PR #142) |
| S28 | Low | both | Any local user could connect to the control kernel's vsock port on QEMU without privilege and, while the host held no connection, control every cell (open finding O8 until then) | Fixed (`authenticate-control-channel`) |

**S1. `mprotect` reached the kernel half (Critical, x86_64).** On x86_64 a
cell's root table shares the kernel's upper-half entries, and those
intermediate entries carry the user bit (x86_64 needs it at every level;
the leaves decide). `mprotect` checked alignment and W^X but not the range,
so `mprotect(0xffff_8000_0000_0000, 1 GiB, PROT_READ | PROT_WRITE)` found
the direct map's 1 GiB page through the cell's table and rewrote its entry
with the user bit. Reproduced in the kernel under QEMU: the cell then read
and wrote physical memory directly. *Fix:* the range must end at or below
`USER_END` (`ENOMEM`, as Linux); `space::protect`, `unmap` and
`map_anonymous` refuse such ranges on their own, user-memory copies refuse
them whatever the tables say, and the copy-on-write walk never enters the
upper half. AArch64 was not affected (the kernel is not in a cell's table).
*Tests:* `mprotect_cannot_open_kernel_pages_to_a_cell`,
`munmap_cannot_remove_kernel_pages`, the property tests, `hostile_ptrs`.

**S12. `mprotect` handed out shared pages (Critical, both).** A fork
shares every page; writable ones are marked copy-on-write, read-only ones
(code, constants) need no mark while nobody can write them. `mprotect` to
`PROT_READ | PROT_WRITE` made such a page writable in place, so the cell
wrote the frame its parent, its siblings, or a template and every cell
later forked from it, still used: code injection across cells.
Reproduced in the kernel under QEMU (`rewrite`: three copies of a forked
cell each saw another's write). *Fix:* `space::protect` makes a page whose
frame is shared writable only as copy-on-write (`PageTable::protect` keeps
or adds the mark), so the first write copies it. *Tests:*
`protect_never_hands_out_a_shared_page` (both formats),
`shared_read_only_pages_stay_shared_after_mprotect`.

**S2. Kernel writes were "resolved" as copy-on-write (High, x86_64).** A
write by cell code to a present kernel page faults with "present, write":
the fault path asked `resolve_write`, which walked the shared kernel tables,
found a writable leaf and answered "already resolved", so the kernel
re-entered the cell at the same instruction, forever (the cell was never
killed; the in-kernel test hung). The walk could also set the writable bit
on kernel intermediate entries. *Fix:* the copy-on-write walk returns "no
page" for any upper-half address. *Tests:*
`kernel_half_is_never_made_writable_or_private`, `poke_kernel`.

**S3. Non-canonical thread pointer (High, x86_64).** The kernel loads a
cell's thread pointer into `FS_BASE` with `WRMSR` before entering it; a
non-canonical value raises `#GP` in ring 0 (Intel SDM, `WRMSR`), a kernel
panic. QEMU's software emulation does not model that check, so this was
confirmed by the host test (the call returned 0 and kept the value) rather
than by a crash; on KVM or hardware it would have crashed. *Fix:* `EPERM`
at or above `USER_END`, as Linux. *Tests:*
`the_thread_pointer_stays_in_the_cell`, `hostile_ptrs`.

**S4. Per-page range walks (High, both).** `space::unmap` stepped through
its range one page at a time, and `mprotect` asked the pager about every
page before checking anything. The kernel runs a call to its end with
interrupts off, so `munmap(4 GiB, 128 TiB)` took 2³⁵ steps: hours with
every cell stopped. Confirmed by the host property test, which did not
finish before the fix (it draws such ranges). *Fix:* `PageTable::hole_end` skips empty space one
empty table entry at a time; `mprotect` stops at the first page it cannot
bring in. *Tests:* `holes_end_where_their_empty_entry_ends`,
`huge_empty_ranges_take_few_steps`, `syscall_storm_does_not_starve_others`
(it hung before).

**S5. Names without limit (High, both).** File data was counted against the
kernel's reserve (H9), but names and inodes were not: a cell creating
directories in a loop grew the kernel heap until it could not grow, and a
heap that cannot grow panics. Names also fed allocations sized by the cell:
`getcwd` of a deep tree, and `getdents64` sized its buffer by the caller's
length. Reproduced in the kernel under QEMU: with the limits below taken
out, cells making names next to a cell taking all memory panicked the
kernel (`memory allocation of 64 bytes failed`). *Fix:* at most 4096 files
and directories per cell (`ENOSPC`);
creating one first asks for memory like a file block does; `getdents64`
builds at most 64 KiB per call. *Tests:*
`files_and_directories_are_limited_per_cell`,
`names_cannot_fill_kernel_memory` (cells make names while another takes all
memory).

**S6. Captured output (Medium).** When the host captures a cell's output,
it was appended to a vector without limit, so a cell printing forever
would fill the heap. Found by reading the code. *Fix:* at most 1 MiB per cell, grown with
`try_reserve`; the rest is dropped (the console still shows it when
echoed).

**S7. The last page of the lower half (Medium, x86_64).** The ELF loader
allowed segments up to `0x0000_8000_0000_0000`. Code ending with a
`syscall` at `0x7fff_ffff_fffe` returns to the non-canonical
`0x8000_0000_0000`: the kernel already avoided `SYSRET` there, but the
remaining `IRET` faults in ring 0 for a non-canonical return address. Only
a host-chosen executable can do this. Found by reading the code (Linux
leaves out this page for the same reason). *Fix:* `USER_END` is
`0x7fff_ffff_f000` for executables, snapshots and copies, as Linux's
`TASK_SIZE_MAX`. *Test:* `the_last_page_of_the_lower_half_is_refused`.

**S8. Snapshot registers (Medium).** `Snapshot::from_bytes` validated pages
and files but not registers. Tampered bytes with a kernel or non-canonical
program counter would make `IRET` fault in ring 0; on x86_64 so would a
non-canonical thread pointer (`WRMSR`) and reserved `MXCSR` bits
(`FXRSTOR`). Found by reading the code. *Fix:* refused with `FormatError::BadState`. *Tests:*
`rejects_what_it_cannot_resume`,
`x86_64_state_that_would_fault_in_the_kernel_is_refused`.

**S9. Console flood (Low).** Unknown numbers below 1024 were reported once,
larger ones every time: a cell could keep the kernel printing to a slow
console. Confirmed by a host test (six reports for seven calls with four numbers). *Fix:*
one report for every number from 1023 up. *Test:*
`unknown_calls_are_reported_once_whatever_their_number`.

**S10. ELF arithmetic (Low).** `phoff + i * phentsize` and the field
readers could overflow on hostile headers. Found by reading the code.
*Fix:* checked arithmetic.
*Test:* `hostile_headers_never_panic` (property).

**S11. Refusing late (Low).** A `mmap` or `brk` larger than free memory
zeroed frame after frame until running out, then gave them all back: all
of free RAM zeroed per refused call. Found by reading the code. *Fix:* ask for the whole length first.

**S13. A cell's flags in the kernel (Medium, x86_64).** An interrupt gate
clears IF and TF but not DF or AC, and the trap entry did not clear them:
after a cell ran `std` and was preempted, the scheduler and every copy it
made ran with the direction flag set, which the Rust ABI forbids (a `rep
movs` copies backwards, corrupting kernel memory); with SMAP on, a cell
setting AC would have switched SMAP off for the kernel. Found by reading
the entry code for `add-user-access-guards`, confirmed by a debug
assertion after every return from a cell (`flags` cell: RFLAGS `0x40400`
in the kernel). *Fix:* `push 2; popfq` right after `SWAPGS` in
`rhizo_user_trap`. *Test:* `flags_a_cell_sets_stay_in_the_cell`.
## Second review (D.2)

Method, as for the first review: a threat model per area, reading the
code, then property tests (fixed seeds, in `cargo xtask check` on every
pull request; random seeds and 4096 cases weekly) and hostile cells in the
kernel; every finding reproduced before its fix and kept fixed by a test.

### Threat model of the new surface

| Area | What a hostile cell (or snapshot) can try | What holds it |
|---|---|---|
| **Several CPUs** (`add-smp-boot`, `add-smp-scheduling`, `add-parallel-threads`) | Threads of one cell on several CPUs racing a system call against `munmap`, `mprotect`, `exit_group` or a futex on memory that goes away; a stale translation on another CPU after a frame is freed | A cell's calls and the faults that change its tables take its lock, one at a time; on x86_64 a shrinking change interrupts the cell's other CPUs and keeps them out until it is done (the clean-CPU set makes others drop the PCID); AArch64 invalidations are broadcast and a copy-on-write replacement is break-before-make (`resolve_write` clears the entry and flushes before writing the new one); the cell is freed by the last CPU to leave it. Per-CPU data is `TPIDR_EL1` / the kernel GS base, out of a cell's reach. Exercised by `racer` (below); no finding. |
| **Sockets** (`rhizo-net`, loopback, Unix, UDP, `smoltcp` on the card) | Exhausting sockets, ports, backlogs and buffers shared by every cell; long or malformed addresses, `msghdr` and `iovec`; `ioctl` | Buffers are bounded (256 KiB per stream, `DGRAM_MAX`) and asked from memory before they grow; paths ≤ 107 bytes; `iovec` counts ≤ 1024 and lengths ≤ `SSIZE_MAX`, their sum saturates (S16); `ioctl` takes `FIONBIO`, `FIOCLEX`, `FIONCLEX` only, on the caller's own descriptor. Ephemeral ports and sockets are a kernel-wide pool (S21); each cell walks the ports from its own start (S25). A Unix name is a number in the cell's own tree: a cell reaches another's Unix socket only if its files hold the name. |
| **Parked connections** (`keep-tcp-across-suspend`, snapshot v6) | A forged snapshot naming another suspended cell's parked connection | Since S15, keys are SipHash-2-4 of a counter under a boot secret no snapshot holds; a key that matches nothing comes back reset or bound again; each key re-attaches once. |
| **virtio** (mmio, pci) | — (the device is trusted, above) | Robustness only: configuration-space parsing bounded (48 capabilities, offsets inside the 4 KiB function), BARs naturally aligned powers of two below the top of memory (S17), structures inside their BAR. Fuzzed (`any_configuration_space_is_set_up_or_refused`). |
| **Dynamic linking** (`PT_INTERP`, file `mmap`, `MAP_FIXED`, the page cache) | Mapping file pages without end; writing through a private mapping of a page other cells share; `MAP_FIXED` over its own or the kernel's memory; a planted interpreter with hostile headers | The cache holds each page it keeps, so a block's bytes cannot change while a frame shows them (a write gives the file a new block, hence a new key); file frames are mapped read-only, writable mappings copy-on-write (S12 rule); a count that cannot grow gives a private copy (S14); `MAP_FIXED` ends at or below `USER_END` and not on page 0; interpreters must be position independent with no interpreter of their own, and every entry point is checked (S18). Mapped holes and pages past the end are not asked for first (S20). |
| **Image cache and 2 MiB pages** (`add-image-cache`, `add-large-image-pages`) | Writing a read-only page or 2 MiB page shared with every spawn of the image | Copies share read-only pages and copy writable ones; `mprotect` of a shared page (2 MiB included) adds copy-on-write; `mprotect` of part of a 2 MiB page is refused; a 2 MiB copy-on-write fault copies into a 2 MiB frame or, without one, into 512 small pages. |
| **Agent workload host calls** (`Host::write_file`, `Host::asleep`, `ioctl`) | — (host calls) | `write_file` reaches a cell only while no CPU runs it (a running cell is out of the scheduler's lists), and goes through the cell's own file limits; `asleep` only reads. |
| **Snapshot files** (`snapshots-on-disk`, format v10) | Any file given to `rhizo load`, as a suspended cell or a template | The control kernel fetches the file over TCP through its network card from the host's listener, reads exactly the announced size (reserved only if cells may take it), and keeps nothing of a refused or cut transfer. Another local process cannot stand in for the guest at the host's listener (B3, B7): each `save` and `load` carries a random one-time token on the serial line, the kernel sends it first on the connection, and `rhizo` takes only a connection from the guest's address that sends it, closing any other before a byte is sent (a loaded file cannot leak) or kept (no forged bytes taken for a saved cell); tested by `transfers_take_only_the_token_s_connection`. `from_bytes` checks the header, then the CRC-32C (damage only: a forger recomputes it), then everything below. A record referring to a frame (`REFERS`) is refused, so a file names no physical address. `Snapshot::cap_quota` lowers the file's quotas to a new cell's (S21's socket share included). Parked-connection keys in a file match nothing in another boot (S15). |
| **Write-or-execute pages** (`node-jit`) | A JIT's writable and executable memory: a page writable and executable at once, one cell changing another's code, a range list without bound, a page that flips forever | The page tables never hold both rights (a property test over maps, protects, forks, clones, writes and fetches on both formats); a change of state makes the cell's tables its own first and copies a frame still shared before writing it; each change flushes the old translation from every CPU that may hold it (broadcast on AArch64, the gang's hold and KPTI's stale marks on x86_64) and, on AArch64, cleans and invalidates the caches before the thread runs again; at most 64 ranges per cell; an instruction that writes its own page ends the cell. Cells may maintain caches by address on AArch64 (`SCTLR_EL1.UCT`, `UCI`), as on Linux: only their own readable memory. |
| **Snapshots** (format v10) | Any bytes as a snapshot | `from_bytes` checks header, version, machine, the checksum, the write-or-execute ranges (at most 64, aligned, sorted, apart, inside the user half), every page record (size, user-only flags, W^X, a write-or-execute page writing or running, alignment, order, inside the user half, data in bounds), every thread (identifier, wait, futex address, program counter and on x86_64 thread pointer and `MXCSR`), the file system (`Fs::decode`: every index, link count, tree shape and bound) and the sockets (each record held by exactly one description). The heap and mapping cursors are not checked: they only steer where the cell's own later calls map, and every mapping call refuses ranges past `USER_END`. Fuzzed with tampered and random snapshots restored into a cell that shares a kernel half (below); no finding. |
| **Cost accounting and sampling** (`add-cost-accounting`, `add-sampling-profiler`) | — | Compiled in only with the `profile` / `sampling` features (benchmark and test builds); the kernel does not grant cells access to the performance counters. Not reviewed further. |

### Findings

**S14. One page shared without end (High, both).** Every mapping of a file
page takes one more reference to the page cache's frame, and the kernel
counts a frame's references in 16 bits with a panic past 65535. O7 rated
that limit low because only the host could reach it; file mappings made it
a cell's: write one byte to a file, map its first page 70000 times.
Reproduced in the kernel under QEMU (`PANIC ... more than 65535 cells
share a frame`). *Fix:* `Frames::try_share` adds a reference only if the
count can hold it; the cache gives the caller a private copy when it
cannot. *Tests:* `a_page_shared_without_end_gets_copies`,
`one_page_mapped_without_end_cannot_stop_the_kernel` (`sharer`).

**S15. Guessable parked keys (Medium, both).** A parked connection was
re-attached by a key counting up from 1 in each boot. A forged snapshot
resumed in the same boot could carry one record per guess and take other
suspended cells' connections: read their peers' bytes and write in their
name. Found by reading the code (design D2 of `keep-tcp-across-suspend`
had accepted it while the host was the only source of snapshots).
*Fix:* keys are SipHash-2-4 of the counter under a 128-bit boot secret
(as strong as the kernel's randomness: O4 until `add-crypto-random`).
*Tests:* `a_parked_socket_answers_only_its_own_key`,
`park_keys_use_siphash_2_4`.

**S16. Buffer lengths that overflow (Medium, both).** `sendmsg` and
`recvmsg` summed up to 1024 lengths of up to `SSIZE_MAX` each with `sum()`,
which panics on overflow when overflow checks are on: debug builds, which
`cargo xtask run` and every in-kernel test use (a release build wrapped,
harmlessly). Confirmed by a host test that panicked. *Fix:* the sum
saturates. *Test:* `buffer_lengths_that_overflow_their_sum_are_harmless`.

**S17. Broken BAR masks (Low, both).** virtio-pci took any value read back
from a BAR as its size mask; one that is not a power of two (or a BAR
whose address plus size wraps) gave sizes that let a structure lie outside
what the device decodes, and an address that could wrap. Out of the threat
model (the device is trusted); found by the configuration-space fuzzer.
*Fix:* such a BAR is left out. *Test:*
`any_configuration_space_is_set_up_or_refused`.

**S18. Entry points past the cell (Medium, x86_64).** S7 kept segments
below `USER_END` and S8 kept a snapshot's program counter there, but an
executable's entry point (`e_entry`), and its interpreter's plus its load
address, were not checked: on x86_64 the first return to the cell would
fault in the kernel for a non-canonical address (as S7). The interpreter
comes from the cell's files, which the cell may have written. Found by the
`exec` fuzzer. *Fix:* `start_at` refuses a first instruction at or past
`USER_END` (`ElfError::BadEntry`), and the executable's bias is added
without overflow. *Tests:* `entry_points_outside_the_cell_are_refused`,
`tampered_programs_start_safely_or_not_at_all`.

**S19. `dup2` to itself (Low, both).** `dup2(fd, fd)` compared and returned
the 64-bit arguments, where Linux takes `unsigned int`: for a valid low
half and a high half set, it returned a value that is neither a
descriptor nor an `errno`. Found by the decoded-files fuzzer. *Fix:* both
truncated to 32 bits, as Linux.

**S20. File holes mapped without asking (Low, both).** A file mapping
gave each hole, and each page past the file's end, a fresh zeroed frame
without asking memory for the whole length first, as S11 made anonymous
mappings do: a call over terabytes zeroed all free memory down to the
kernel's reserve (kept by the frame allocator) before it was refused and
gave everything back. The frames came from the page cache's pool, outside
the cell's metering. *Fix* (`add-cell-quotas`): such a page is the cell's
alone and never kept, so it comes from the cell's frames and counts in its
memory quota like anonymous memory. *Test:* `hog file` in
`memory_quota_refuses_the_cell_not_the_others`. A cell without a memory
quota is still bounded only by the reserve, as for anonymous memory.

**S21. One pool of sockets (Medium, both).** At most 4096 sockets are
open in the kernel. A cell with 1024 descriptors could open as many
connections to its own listener (each also holding the accepted end), so
one cell held about half the pool and two held all of it; parked sockets
count too. Others then got `ENFILE`; the kernel survived. *Fix*
(`add-cell-quotas`): every cell has a socket quota, 256 unless the host
sets another (`Quota::DEFAULT`), so with the server ends of its own
connections a cell holds at most 512 sockets, an eighth of the pool (a
compile-time check keeps the default there). *Test:*
`a_cell_holds_a_bounded_share_of_sockets` (`limits` and its fork, without
a quota from the host). A host that lifts the quota (`Quota::NONE`) or
runs many cells can still fill the pool, as with memory.

### Fuzzers and hostile cells

| Fuzzer | Input | Checks |
|---|---|---|
| `x86_64_cells_cannot_hurt_the_kernel`, `aarch64_...` (first review) | Up to 40 calls of any number and arguments | Values or errnos, kernel memory and tables unchanged, bounded reports, every frame back |
| `decoded_files_cannot_hurt_the_kernel` | A file system with every kind of object, encoded, up to 3 bytes changed and maybe cut, then up to 30 calls on descriptors and paths | Refused, or the same as above with the decoded files |
| `random_bytes_are_refused_or_work` | Random bytes as an encoded file system, then calls | As above |
| `tampered_snapshots_are_refused_or_contained` | A snapshot with threads, pages (two write-or-execute, writing and running), a write-or-execute range, files, a pipe and two sockets, up to 3 bytes changed (mostly in headers and records) and maybe cut, its checksum made right again in 9 cases out of 10 (`snapshots-on-disk`) | Refused, or restored into a cell sharing a kernel half: kernel memory and entries unchanged, every page in the user half with user flags and W^X, ranges below `USER_END` and at most 64, registers below `USER_END`, every descriptor answers, sockets import, every frame back |
| `random_bytes_are_refused_or_contained` | Random bytes behind a valid header, with a valid checksum | As above |
| `tampered_programs_start_safely_or_not_at_all` | A dynamically linked program and its interpreter with up to 3 bytes each changed in their headers | Refused with every frame back, or started with every page in the user half, W^X, a first instruction below `USER_END`, every frame back |
| `any_configuration_space_is_set_up_or_refused` | A QEMU virtio-net function with up to 5 dwords changed, any BAR sizes, any window | No panic; structures inside a BAR, no wrap |
| `hostile_headers_never_panic` (first review) | ELF headers | No panic |

In the kernel, on both architectures and with 4 CPUs (`ktest-smp`):
`one_page_mapped_without_end_cannot_stop_the_kernel` (`sharer`) and
`threads_racing_on_several_cpus_cannot_hurt_the_kernel` (`racer`: copies
through the kernel racing `munmap` and `MAP_FIXED` 2000 times, a futex
wait on a word unmapped under it, `exit_group` while three threads are in
system calls, `mprotect` flipping a page under a writer); each ends as its
race allows, a neighbour finishes, every frame comes back.

Limitations of this review: fuzzing is by property tests on the host
libraries with test doubles, not coverage-guided; the kernel's own wiring
(`kernel/src/cell.rs`, the scheduler loop, the entry code, the TLB
shootdown protocol) was read, and exercised by hostile cells, not
fuzzed or proven; races are tested by running them, which shows a bug only
when the interleaving happens; the in-kernel tests run under QEMU with
software emulation in CI (and KVM where available), whose memory ordering
is stronger than some hardware's.

## Frames from the network (B5)

Change `fuzz-wire-packets`, answering Q11 of
[`audit-scope.md`](audit-scope.md): the third attacker (a host on the
network, above) against `crates/rhizo-net/src/wire.rs`, `kernel/src/wire.rs`
and the parts of `smoltcp` 0.14 the kernel builds (Ethernet, ARP, IPv4,
ICMP errors, UDP, TCP, the DHCPv4 client). Method: reading the code, then
a host fuzzer that drives `Net::poll_wire` through the `QueueLink` test
double (below); each finding reproduced by a test that failed before its
fix.

### What a host on the network can try

| Input | What holds it |
|---|---|
| **Floods** of any frames | One poll takes at most one receive ring (64 frames) since S22, then asks to be polled again; the kernel polls at most every 20 µs, so cells run in between. |
| **SYN floods**, SYN to a cell's listener | Listening sockets are loopback only: `listen` never makes a `smoltcp` listener, so a SYN to any port gets a reset and leaves no state. Fuzzed: nothing ever waits to be accepted. |
| **TCP segments**: malformed headers and options, out of window, aimed at another cell's port | `smoltcp` checks lengths and checksums, then matches the four addresses and ports: a segment that is not from a connection's peer gets a reset, and a connection's state changes only for segments from its peer (D3 of the change: a host that can forge the peer's addresses is out of reach of any stack). Handshake options (zero MSS, window scale past 14, short or unknown options) fuzzed on a connection in `SYN-SENT`. |
| **UDP** | Only a socket that sent to the outside has a `smoltcp` socket, bounded to 16 datagrams and 64 KiB: anyone may fill that socket's own queue, as on any stack; nothing else receives. Datagrams to other ports get an ICMP error quoting the sender's own datagram. |
| **IPv4 fragments and options** | `smoltcp` is built without `proto-ipv4-fragmentation`: fragments are dropped, no reassembly buffer exists. Options are skipped. |
| **Oversized frames** | The driver copies at most `FRAME_MAX` (2036 bytes) of a buffer; `smoltcp` checks every length against the bytes it got. Jumbo frames fuzzed. |
| **ICMP** | No echo replies (feature `auto-icmp-echo-reply` off). Frames the card sends are built in zeroed buffers (`Tx`), so no stale heap bytes leave. |
| **ARP** | A neighbour cache of 8 entries, filled only by ARP aimed at the card from its own network. A host can evict entries; `smoltcp` then asks again (one request a second), which delays a connection's frames by about a second (below). |
| **DHCP** | A rogue server can give the card any unicast address or take it away (a NAK, a short lease): connections from the old address then fail. Inherent to DHCP; fuzzed for panics and for non-unicast addresses. |
| **Memory** | In this configuration `smoltcp` allocates nothing per frame (fixed socket buffers, fixed-size neighbour cache); `Net` reuses one frame buffer per poll. Fuzzed: the heap comes back to its level once every socket is closed. |

### Findings

**S22. A flood held the polling CPU (Medium, both).** `Net::poll_wire` let
`smoltcp`'s `Interface::poll` take frames until the card had none (its
documentation warns that this is unbounded), and the virtio-net driver
gives each buffer back to the device as it hands its frame over. A host
sending faster than the kernel handles frames kept the CPU that polls the
card in the kernel for as long as the flood lasted: the cells it runs did
not run, and the cells' own frames waited (egress comes after ingress).
Found by reading `smoltcp` while writing the fuzzer's flood step;
reproduced by a card that always has another frame (the test panicked
after 100000). *Fix:* the device `smoltcp` sees hands over at most
`POLL_FRAMES` (64, one receive ring) per poll, and `poll_wire` then
returns 0 so the caller polls again soon. *Test:*
`a_flood_from_the_wire_lets_the_kernel_go`.

**S23. Idle connections to the outside were reset after 20 s (Low,
both; `smoltcp` 0.14).** Connections to the outside were given
`set_timeout(20 s)`. `smoltcp` documents that
timeout for a peer that does not answer the handshake or stays silent
while data is outstanding, but its `poll_at` comes due at the last
received segment plus the timeout whatever the transmit buffer holds, and
`dispatch` then aborts the connection (`timed_out` does not look at the
buffer). So any connection on which nothing arrived for 20 s, such as an
idle keep-alive connection or a request whose answer took longer, was
reset and its peer got an RST. Not caused by an attacker; found by the
fuzzer (a case that shrank to "wait 20 s"), and at the same time, from a
model that thought for 45 s, by `fix-tcp-long-wait`. Reproduction in `smoltcp`
alone: two interfaces on a wire, a client socket with
`set_timeout(Some(20 s))` connects to a listener, neither sends; 20 s
after the last segment the client sends an RST and is `Closed`.
`smoltcp` is a crates.io dependency and is not patched. *Workaround*
(`fix-tcp-long-wait`): the card clears the timeout while an open
connection has nothing waiting for an acknowledgment and sets it again
when it sends. S23 is covered by that change's dedicated tests in
`crates/rhizo-net/src/wire_tests.rs`:
`a_connection_silent_for_long_still_gets_its_reply` (45 s of silence, then
ten minutes idle) and `bytes_a_peer_never_acknowledges_are_given_up`.

### The fuzzer

In `crates/rhizo-net/src/hostile.rs`, run like the other fuzzers (fixed
seed in `cargo xtask check`, random seed and 4096 cases weekly):

| Property | Input | Checks |
|---|---|---|
| `frames_from_the_wire_cannot_hurt_the_kernel` | Up to 40 steps next to two cells' sockets (A: a connection to a well-behaved peer and a loopback connection; B: a connection to the attacker's stack, a connection whose handshake the test answers itself, a UDP socket, a listener): time passing, the cells sending, valid frames (SYN to the listener, to closed ports and to a loopback address, odd TCP options, segments to B's connections and to A's port, SYN-ACK and data on B's crafted connection, UDP to B's socket, to closed ports, broadcast and jumbo, IPv4 fragments and options, ICMP, ARP, a DHCP offer, other Ethernet types) with up to 3 bytes changed, maybe cut, checksums mostly fixed; the attacker's own frames replayed and changed; random bytes; floods. Frames claiming the peer's MAC or IP address are not sent. | No panic; always 8 sockets open and nothing to accept; no frame holds the loopback bytes, and only frames to the peer hold A's; A gets back exactly every byte it sent and keeps its addresses, with no error; once every socket is closed and 75 s pass, nothing is open, nothing is left in `smoltcp`, and the heap (counted per thread by the test binary's allocator) is back to its level |
| `dhcp_replies_cannot_hurt_the_kernel` | A card asking for a lease: up to 30 steps of time passing, DHCP replies (offer, ACK, NAK or any type, any address and lease) to the client's last transaction with bytes changed and cuts, random frames | No panic; the address, if any, is unicast; the heap is back to its level |
| `seeds_reach_what_they_aim_at`, `a_valid_lease_configures_the_card` | The seeds unchanged | Each reaches its target (a reset, an ICMP error, an ARP reply, a datagram, a handshake, data in order, a lease), so changed copies test the paths behind the first checks |

Run it:

```sh
cargo test -p rhizo-net -- hostile::
# Longer, with a new seed (the change's run: three seeds, 45 minutes each):
PROPTEST_RNG_SEED=$RANDOM PROPTEST_CASES=100000 cargo test -p rhizo-net -- hostile::
```

A failure is saved under `crates/rhizo-net/proptest-regressions/` and
replayed first on the next run; commit it with its fix.

Limitations: property tests on the host, not coverage-guided, against
`smoltcp` as configured in the kernel; the virtio rings themselves are B6
(a trusted device). A host on the VM's network segment can still evict
neighbour-cache entries and delay connections by about a second, and a
rogue DHCP server can change the card's address: both come with sharing a
network segment, and are listed below.

## Cross-cell leaks

Change `close-cross-cell-leaks`, from the covert-channel inventory of
`add-cell-hierarchy` (design D12 there: 42 kernel resources more than one
cell can touch). Goal 6 of the threat model: two hostile cells holding no
handle to each other, one changing kernel state, the other reading it
(storage channel) or timing it (timing channel). The hierarchy closes most
of the inventory in phases; this change closes the three items that leaked
today at little cost. Method: reading every call that reports kernel
state, and a hostile in-kernel test per finding with two cells
(`observer`, `hog`), each failing before its fix.

### Findings

**S24. `sysinfo` showed the machine (Medium, both).** `totalram` was the
memory cells share and `freeram` the frames free in the allocator: a cell
reading it in a loop saw every other cell allocate and free, and two cells
could signal through it at many bits per second. *Fix:* `sysinfo` reports
the calling cell only: its memory quota (at most the memory cells share)
as `totalram`, what its own metered use leaves of it as `freeram`, its
threads as `procs`; no load, no swap. Nothing else reports global state:
there is no `/proc`, `getrusage` and `times` answer `ENOSYS`, `getrlimit`
and `prlimit64` report the cell's own quota, `uname` and
`sched_getaffinity` are constant, the clocks are boot-relative.
*Tests:* `sysinfo_shows_a_cell_only_itself` (in the kernel: a neighbour
holding 32 MiB changes nothing in an observer's `sysinfo`; it read
`change=32` before), `sysinfo_shows_only_the_cell_itself` (`rhizo-abi`).

**S25. One counter for all ephemeral ports (Medium, both).** `bind` to
port 0 and implicit binds took the next port of one counter: the distance
between two ports one cell took was one plus every port other cells took
in between, connections to the outside included. *Fix:* each cell walks
the range in order from a start of its own, SipHash-2-4 of its
identifier under the kernel's secret (with the boot, as
`fix-ephemeral-ports` wanted); the cell's system-call handler keeps its
cursor and hands it to the sockets for the five calls that can bind. A resumed or forked
cell has a new identifier, so a new start. *Tests:*
`ephemeral_ports_do_not_show_other_cells_binds` (in the kernel: a
neighbour binds 100 ports between an observer's two binds; the observer's
ports are 1 apart, 102 before), `a_cells_ephemeral_ports_do_not_show_other_cells_binds`
and `a_cells_ephemeral_ports_skip_ports_in_use` (`rhizo-net`),
`another_cells_binds_do_not_show_in_my_ports` (`rhizo-abi`).

**S26. Any cell could empty the page cache (Medium, both).** The cache of
file pages (`add-dynamic-linking`) emptied itself when a page came past
its 65,536 pages, which a cell reaches by writing 256 MiB of its own
files and mapping them, and `drop_images` emptied it under memory
pressure, which a cell without a memory quota reaches by allocating.
Another cell's next map of a library page then missed: the flush of a
flush-and-probe channel. The frames of pages still mapped also left the
cache for their mappers without being counted against their quotas, so a
cell could map file pages past its quota by flushing and mapping again.
*Fix:* a full cache drops only pages whose block no file holds any more
(nobody can map them again), at most once every 4,096 refusals while it
stays full, and a page it has no room for is the caller's own copy from
its own frames, counted in its quota (as is now the copy S14 gives).
Memory pressure drops only kept pages whose dropping frees memory: those
no mapping uses and those no file holds. *Tests:*
`memory_pressure_keeps_the_pages_other_cells_map` (in the kernel: `hog`
exhausts memory while `observer page` maps a page of its own file; the
page stays kept, where the cache was empty before the fix),
`a_full_cache_keeps_what_others_map` and
`a_full_cache_drops_pages_nobody_can_map` (`rhizo-abi`),
`a_page_knows_whether_its_block_is_held_elsewhere` (`rhizo-fs`).

### What is left

The items this change left open (D12 numbers of `add-cell-hierarchy`:
ports as one namespace, page-cache deduplication, identifiers, shared
pools) are closed by phase 1 of `add-cell-hierarchy`, for domains whose
roots are reserved; timing is measured. See
[Colluding cells](#colluding-cells-add-cell-hierarchy-phase-1).

## Colluding cells (`add-cell-hierarchy`, phase 1)

Phase 1 of `add-cell-hierarchy` puts cells in a tree (design D1 to D4
there; a summary in [`cell-hierarchy.md`](cell-hierarchy.md)). A
**domain** is a root and its descendants, copies of them included: the
unit the host isolates, its tenant. This section reviews what the phase
closes against cooperating cells, with the hostile tests of task 1.12.

### Threat model of colluding cells

**Attacker** (design D0). Two or more cooperating agents, each in its own
cell, which know the kernel's source. They want to:

1. communicate when nobody gave them a channel: directly (a shared
   loopback port), through storage channels (state one changes and the
   other reads: a port in use, free memory, an error code, a file name) or
   through timing;
2. make the network card send bytes where their egress policy forbids,
   themselves, through a cell with more egress, or with a socket such a
   cell opened;
3. gain authority nobody granted: control over a cell that is not a
   descendant, wider egress, a larger quota, by re-parenting, promotion,
   forks, snapshots or forged numbers;
4. exhaust shared resources so that other domains fail (fork bombs, the
   socket pool, ports, the kernel heap).

**Second attacker:** a forged snapshot file, which must bring no
authority: no tree position, no wider policy, no larger limit.

**Trusted:** the host (the `rhizo` server and whoever drives it), as in
the rest of this review.

**What is promised**, strongest first:

- **G1. No authority without a grant.** Every way to reach another cell
  and every socket to the outside comes from the creator or the host. In
  phase 1 there is no way to reach another cell at all: each has a
  loopback network of its own and handles come with phase 2. Sockets
  never move between cells.
- **G2. Egress per socket**, from the policy of the cell that made it,
  `none` by default, under a deny floor holding the host's own
  addresses. A cell with `none` never puts a packet on the wire, whatever
  another cell tells it.
- **G3. No storage channel between domains** whose roots are reserved
  (have a memory limit): apart from elapsed time, nothing a cell observes
  depends on what another domain does.
- **G4. Timing channels are reduced, listed and measured**, not closed:
  on shared CPUs and caches they cannot be. They carry information
  slowly, never a handle, a socket or reach.

So two colluding domains can exchange information at the bandwidth of
the timing channels below, and reach together at most the union of their
own egress policies.

### The covert-channel inventory

Every kernel resource more than one cell can touch (design D12 of
`add-cell-hierarchy`, 42 items), with what phase 1 did about it.
"Within a domain" items are accepted: the domain is the unit the host
isolates, and two agents that must not collude belong in two domains.

| D12 | Shared resource | Storage | Timing | Status after phase 1 | Kept by |
|---|---|---|---|---|---|
| 1, 2 | Loopback network, its ports and port counter | Was direct communication; `EADDRINUSE` | — | **Partitioned**: a network per cell, never shared (D5), ports and counter per network | `separate_loopback_networks`, `colluders_on_loopback`, `differential_colluders` |
| 3 | Card ports | Was the loopback counter | — | **Eliminated**: card ports drawn at random by the kernel, shown to no cell (D6) | `ports_of_other_domains_do_not_show` |
| 4 | Unix socket names | Not observable; forged numbers | — | **Partitioned**: keyed by (network, name) | `colluders_on_loopback` |
| 5 | Socket pool (4096) | `ENFILE` | — | **Partitioned**: reserved socket limits admitted against half the pool | `differential_colluders` |
| 6 | Free memory | `sysinfo`, `ENOMEM` | Allocation time | **Eliminated** for reserved roots: `sysinfo` shows the cell's limit, a reserved root never fails because of others (D4). Unlimited roots share what is left: accepted | `sysinfo_shows_a_cell_only_itself`, `a_reserved_root_keeps_its_memory`, `differential_colluders` |
| 7, 8 | Kernel heap, file data | `ENOSPC`, `ENOMEM` | Fragmentation | **Eliminated** (storage): file data, names, thread and socket records and queues charged to the cell's limit | `files_count_in_the_memory_limit`, `differential_colluders` |
| 9 | Page cache | — | Hit or miss, flush | **Partitioned** per domain, bounded and charged per domain; public pages loaded at publication, never evicted (D21) | `file_pages_kept_per_domain`, `same_file_in_two_domains`, `public_library_always_hits` |
| 10 | File blocks shared by reference | — | Sole-owner writes | No channel across domains (only the host's file sets are shared, and the kernel holds them) | — |
| 11 | Image cache | — | Spawn time | Only the host spawns from images: no cell-driven state | — |
| 12 | Template and snapshot frames | — | Sole-owner writes | **Eliminated**: only published templates have copies in several domains, and they keep their frames until their last copy ends | `template_dropped_under_two_domains` |
| 13 | Frames of host forks | — | Same | Within a domain: accepted | — |
| 14 | Free 2 MiB blocks | — | Large-page fallback | **Accepted** | — |
| 15 | CPUs and scheduler | — | **High** | **Accepted, measured**: `covert.cpu_bps` below; exclusive CPU sets (D17) take it out | `exclusive_sets_hold` |
| 16 | Caches, TLB, predictors, memory bus | — | High | Out of scope (O3); CPU sets reduce private-cache sharing | — |
| 17 | `CNTVCT_EL0`, `rdtsc` | — | The stopwatch | **Accepted** | — |
| 18 | Monotonic and real time | — | — | No channel: no cell changes them | — |
| 19, 20 | Cell and thread identifiers | Counted every cell (O6) | — | **Eliminated**: numbered from 1 in each domain, a copy unlike its source | `identifiers_are_per_domain`, `differential_colluders` |
| 21 to 23 | Control-plane identifiers, output relay, console reports | Host only | — | No channel; a host that pipes one domain's output into another builds the channel itself | — |
| 24 | Error codes of shared pools | Yes | — | **Eliminated**: each depends on the domain's own state | `differential_colluders` |
| 25 | Network card: MAC, IP, ARP, DHCP, queues | Constants | Neighbour cache, queues | **Accepted**; cells with egress `none` never touch the card | — |
| 26 | DNS | — | — | Not in the kernel: a resolver is a destination of the policy (and a way out in names, which `rhizo` warns about) | — |
| 27 | Socket wait queues, change counter | — | — | No channel | — |
| 28 | Kernel locks | — | Wait time | **Accepted** | — |
| 29 | Cell table | Refusal when full | — | **Partitioned**: descendant limits carved from the parent and admitted against the table | `fork_bomb_in_a_subtree` |
| 30 | Random generators | — | — | No channel; copies learn to reseed from their copy generation (D22) | `copies_draw_different_random` |
| 31 | Parked socket keys | — | — | Unguessable (S15) | — |
| 32 to 34, 39 | Grant table, channel queues, futexes, `/run/rhizo` | — | — | No channel (handles and channels are wired in phase 2) | — |
| 35, 36 | Constants, the host's file sets and templates | — | — | Read-only; see 10 and 12 | — |
| 37 | Exit status | Authorized (tree edge) | — | Flow edge | — |
| 38 | File names | None | — | **Confirmed**: each cell has its own tree; `rename` changes only the caller's. No shared namespace to signal through | — |
| 40 | Snapshots on disk | Host mediated | — | Loaded snapshots carry no authority (D13) | `forged_snapshot_authority` |
| 41 | Accelerators | — | — | Not built (`add-accelerators`) | — |
| 42 | CPU weights and caps (D23) | — | A domain's share depends on others' load | Like 15: accepted; caps bound what a domain takes | `domains_share_by_weight`, `cpu_cap_holds` |

**What remains is timing.** Items 14 to 17, 25, 28 and 42 cannot be closed
on shared hardware. One of them is measured end to end by `cargo xtask
bench` (`covert.cpu_bps`, reported and never gated): a sender cell in one
domain spins through a period of 40 ms (four time slices) for a 1 and
sleeps through it for a 0, a receiver in another domain counts the loops
it makes in each period and takes those below the median of the nine
periods around them for 1s; the figure is the capacity of what it
decoded (`(1 - H(p))` bits per period, `p` the share of wrong bits over
100; 25 bits per second when every bit is right), first with both on one
CPU, then with an exclusive set of one CPU each (`covert.cpu_sets_bps`).

| Measured (4 vCPUs, x86_64, QEMU `microvm`) | `covert.cpu_bps` (one CPU shared) | `covert.cpu_sets_bps` (exclusive sets) |
|---|---|---|
| Without KVM (TCG, vCPU threads not pinned, 4-CPU host) | 10.4 (14 % of bits wrong) | 11.1 |

These are the first figures, from emulation; `bench.yml` measures both
under KVM on every run on `main`, and this table is to be replaced by
those. What they show already:

- On a shared CPU the channel works: about 10 bits per second with this
  crude sender, against the 25 its period allows. A better coder (shorter
  periods, error correction) gets more; the literature reports hundreds
  to thousands of bits per second for contention channels.
- Exclusive sets take the kernel's scheduler out of it (no cell of
  another domain runs on a set's CPU, `exclusive_sets_hold`), but under
  emulation they did not lower the figure: the vCPUs are threads of the
  host, and a spinning vCPU slows its neighbour through the host's cores
  and caches. That is the hardware sharing D17 leaves to the VMM (each
  vCPU pinned to a physical core of its own, `rhizo up --pin`); whether
  pinned vCPUs under KVM bring the figure to noise is what the KVM runs
  must show.

Inside one VM, colluding agents in different domains can still signal
each other slowly through timing; they cannot gain reach, handles or
quota by it. Separate VMs or hosts are the answer for more.

**Memory the kernel still takes unmetered.** Task 1.2 charges file data,
names, records, queues and network buffers to the cell, but four things
stay outside every limit, taken from the memory no reservation holds:
copy-on-write copies of frames shared by a fork or a template, the pages
a domain keeps in the page cache, pipe buffers, and the room a channel
reserves. A reserved cell's first write to a page it shares with its
source can therefore still be refused when cells without a limit have
exhausted that memory, and so can its writes into pipes: a fault or an
`ENOBUFS` that depends on another domain. Open as O9.

### Content sharing stays in its domain (D21)

A frame is shared by its contents (the same bytes mapped from one frame)
only between cells of one domain: the domain is part of every content
key. That covers the page cache of mapped files (executables and
interpreters included) and the frames a snapshot refers to instead of
copying. Across domains only **public objects** are shared: immutable,
holding no secret, published by the host (`rhizo publish`): the host's
file sets, read-only template files and published templates. Their pages
are loaded when they are published and kept until they are withdrawn and
unmapped, so a map of a public page always hits and residency tells no
cell what another mapped. Nothing is ever merged by scanning memory (no
KSM). A cell moved into another domain, or a suspended one resumed there,
first takes private copies of the non-public frames it shared, or the
move is refused. This closes the deduplication and flush-and-reload
attacks of D21 (Flip Feng Shui, deduplication timing, prompt-cache
timing) between domains; within a domain sharing is intended.

### What copies keep of their template (D22)

The kernel's generators are per cell and never copied, but generators in
user space live in memory and a copy duplicates them. Each cell has a copy
generation (0 at spawn, incremented in every copy: host forks, template
copies, resumes and loads), readable in `/run/rhizo/generation`;
`MADV_WIPEONFORK` ranges read as zero in every kind of copy; and a copy's
`getpid` differs from its source's, which OpenSSL 3 and CPython's
`tempfile` take as a fork. The shipped CPython and Node.js templates
reseed after a copy. What every copy still shares with its template, as
any fork server does: CPython's string hash seed, glibc's stack-protector
value and pointer guard, and the address layout. A server that hashes
untrusted keys from another domain should start from its own program,
not from a template. Templates must hold no secrets: the host passes them
to a copy after the copy, and the kernel refuses to publish a template of
a cell that reached the network or received a message unless the host
attests it (audited).

### Hostile tests (task 1.12)

In the kernel, on both architectures, with the colluder cells of
`cells/src/bin/colluder.rs` (`kernel/src/tests/collusion.rs`):

- `differential_colluders`: a receiver, a reserved root of 12 MiB and 16
  sockets with two children, runs a fixed probe twice once the host wakes
  it: identifiers, `sysinfo`, four threads, memory to its limit, a file
  to its limit, sockets to its limit, a listener, a bind conflict, a
  connection and a datagram on loopback ports 7400 to 7402, three
  ephemeral ports (as distances), a round trip with the echo server
  through the card, and a file mapped twice. Then the same beside a
  sender's domain of the same size, whose root, two children and copies
  of the root (as many as the kernel admits, at most two) do all of it as
  hard as they can, on the same ports, holding what they took across
  yields. The receiver's and its children's outputs are byte for byte
  the same, the senders made rounds during the probe, and after the
  hostile rounds every frame comes back.
- `reserved_probe_beside_an_unlimited_hog`: the receiver's probe, in a
  reserved root of 12 MiB, prints the same alone and beside a cell
  without a limit that took every frame it could (finding S30).
- `colluders_on_loopback`: a listener on TCP and UDP port 7500 of every
  address and on a Unix socket; a knocker of another domain with egress
  `full` gets `ECONNREFUSED` on 127.0.0.1:7500, its datagram goes
  nowhere, the Unix name does not exist in its tree, the card's own
  address does not loop back to the listener, it binds port 7500 itself,
  and the listener heard nothing.
- `fork_bomb_in_a_subtree`: under a root of 64 MiB and 8 descendants, the
  host forks every cell of the subtree and places new cells under each,
  round after round: the subtree never holds more than 8 descendants,
  the rest are refused for the budget, a cell of another domain runs to
  its end, and every frame comes back.
- `forged_snapshot_authority`: a snapshot sealed again after its tree
  section was changed to claim egress `full`, 4 GiB, every CPU and
  unlimited descendants. Loaded as a template under a parent with egress
  `none`, it and its copy have egress and ceiling `none`, the host's
  limits, no CPU set, and their connection outside is denied; resumed as
  a root, the 4 GiB are refused, and with a claim the kernel can hold it
  still has egress `none` and no CPU set.

### Findings

**S29. A file claiming large limits could not be loaded under a parent
(Low, both).** The control kernel enters a loaded snapshot in the tree as
a root before it places it where the host asked, and that root took the
limits its bytes claimed, admitted against the kernel's free memory. A
file claiming more memory than the kernel could set aside (a forged one,
or one saved on a larger machine) was refused (`the template does not
fit`) even under a parent with room for the limits the host gave it:
no authority was gained, but a placement the spec allows failed, and a
file could make the host's request fail by what it claims. *Fix:* a
snapshot the host places under a parent enters as a root with a new
cell's limits (`cell::to_place`, `Snapshot::set_quota`), and its
placement alone gives its limits. *Test:* `forged_snapshot_authority`.
A file loaded as a root keeps the memory limit its bytes ask for (capped
by `cap_quota`), which makes it a reserved root admitted like any other:
it can never set aside more than the kernel has, and the host chooses
what to load.

**S30. Another domain could make a reserved cell fail, or a cell pass its limit
(High, both).** Kernel memory a cell's calls take (a file name, a new
file block, a socket, a thread record, a received descriptor) was counted
against its limit and then also asked of the kernel's memory outside every
reservation, and so was a write into a block the cell already held and a
send into queues counted with the socket. Cells of other domains can
exhaust that memory: cells without a limit, and, in a reserved domain,
the file pages its cells keep in the page cache and copy-on-write copies,
which no limit counts (O9). A reserved cell then got `ENOSPC`, `ENOMEM`
or `ENOBUFS` for memory its own limit had set aside: a storage channel,
and a breach of "a reserved root never fails because of others".
`differential_colluders` found it on four CPUs, where the receiver could
not create a file while the sender's domain mapped its file pages: the
receiver's output differed from its run alone. *Fix:* for a cell with a
memory limit, what its meter counts is its own, set aside within its
limit, and the kernel is asked only for cells without one
(`Context::has_counted` in `rhizo-abi`). Pipe buffers, which no limit
counts yet, still ask the kernel (O9).

Two more holes of the same kind showed once the test put a cell without a
limit, holding every frame it could take, beside the probe (and on Cloud
Hypervisor with 4 CPUs, where the receiver's run alone could not take any
of its memory in its second round):

- *Frames a reservation counts could be gone.* A reserved cell's frames
  came from the free frames beyond the kernel's own reserve and every
  reservation. But the kernel heap draws on that reserve for what no meter
  counts (cell output, the kernel's tables, each meter's rounding to whole
  frames), so once cells without a limit had taken the rest, a reserved
  cell was refused frames its limit had set aside. *Fix:* a meter with a
  limit asks for its frames as set aside (`Frames::alloc_set_aside`), and
  the kernel serves them from its own reserve once nothing else is left
  (`memory::alloc_set_aside`), keeping only what other domains hold.
- *Unmapping a cached file page credited the cell.* A cell's meter
  credited every frame reference it gave back, including references to
  the kernel's page-cache frames it had mapped without being charged. A
  cell mapping and unmapping a file page lowered its count every time, so
  it could hold more than its limit; and whether its map was a cached page
  (credited, never charged) or a copy of its own (charged, then credited)
  depended on whether the cache had room, so on other domains: the probe's
  `sysinfo` and the memory it could take differed beside the hog. *Fix:*
  giving back a reference to a frame the kernel keeps credits nothing
  (`Frames::release`; the kernel marks the frames its page cache keeps
  beside their reference counts).

*Tests:* `a_reserved_cell_stores_within_its_limit_when_the_kernel_is_short`
(`rhizo-abi`), `references_to_kept_frames_credit_nothing` (`rhizo-cell`),
`reserved_probe_beside_an_unlimited_hog` and `differential_colluders` (in
the kernel; the hog test failed before both kernel fixes, at the probe's
file mapping and then on its `sysinfo`).

## The control channel over vsock (B7, B8)

Change `control-over-vsock` moved the control kernel's frames (requests,
replies, cell output) from the serial line to a virtio socket device when
the VMM gives one: always on Firecracker and Cloud Hypervisor, and on QEMU
when the host's `/dev/vhost-vsock` is usable (policy below). Otherwise
they stay on the serial line, which carries the console's text in every
case.
Code: `crates/rhizo-virtio/src/vsock.rs` (the `Stream` protocol and the
`VirtioVsock` driver), `kernel/src/vsock.rs` (the device, polled from the
control loop) and the `link` module of `kernel/src/control.rs`. Only
control builds have it. The kernels that run the in-kernel tests and the
benchmarks have no socket device. Method: reading the packet parsing and
the driver as an auditor, and checking what the existing tests already
cover; each finding was reproduced by a test.

### The boundary

The host is trusted (B7), and the frame decoder (`rhizo_control::Decoder`)
was already treated as an attack surface. The vsock layer adds a packet
protocol under it, which parses everything as hostile:

| Input | What holds it |
|---|---|
| **Packets from the host side**: any addresses, socket type, operation, length and flags | `Stream::take` holds one connection. It is made by a `REQUEST` to the stream's own CID and to port 1024 while no connection is held. Any other packet gets a reset, except a reset itself: a second connection, another port, another socket type, a stranger. On the connection, a `SHUTDOWN`, another socket type, or a stated length that is not the payload's resets it. Unknown operations are ignored. At most 64 answers wait to go out, so a flood cannot grow memory. Property test `hostile_packets_are_survived`. |
| **Credit**: `buf_alloc` and `fwd_cnt` of any value, counters that wrap | The stream sends at most `buf_alloc` less what it sent and the peer has not acknowledged. The subtraction wraps and the result saturates at zero, so a peer that claims to have read more than it got gets no credit at all, never extra. A peer that sends past the 256 KiB the stream advertises is reset, so at most 256 KiB is ever held unread. More than 4 MiB written and not sent (a peer that stopped reading, or gives no credit) resets the connection. Property test `credit_is_never_exceeded`; tests `a_peer_that_breaks_the_protocol_is_reset`, `a_peer_that_never_reads_cannot_fill_memory`. |
| **Resets and shutdowns** | They close the connection and drop both buffers. Until S27 (below), a connection that replaced the previous one before the kernel looked was missed. |
| **The device**: used-ring entries, lengths, descriptor numbers | A used buffer shorter than a header is skipped, a length beyond the buffer is clipped to it, and a descriptor number is folded into the ring. A transmit slot returned twice is taken back once. No buffer outside the driver's own is read or written, and each packet is copied out before its buffer goes back to the device. Test `a_hostile_device_cannot_overrun_buffers`. |
| **CIDs and ports** | The kernel reads the guest's CID from the device's configuration, so the host chooses it: 3 plus the `xtask` or `rhizo` process identifier shifted left by 6, plus a counter. That is unique on the host, as QEMU's vhost requires, but anyone can work it out (S28: a connection must present the boot's secret). The stream accepts a request from any source CID. Today that has no effect, because the VMMs deliver only the host's packets to a guest: vhost-vsock, Firecracker and Cloud Hypervisor have no guest-to-guest path. Checking for CID 2 would be defence in depth. In the other direction, the guest chooses the source CID and port of the packets it sends, and the VMM is trusted to drop a forged source (B9). The host listens on no vsock port, so a guest can open no connection to the host. |

Cells never reach the device. Their output still reaches the host only
inside frames (B8, Q12 of the audit scope). A cell that prints without end
cannot make the kernel reset the host's connection through the 4 MiB
limit, because `xtask`'s reader thread drains the connection into an
unbounded queue. The cost is on the host: the `rhizo` server's memory
grows while a slow client is attached. That was already true on the
serial line, only slower.

### The host's `/dev/vhost-vsock` policy

QEMU needs `/dev/vhost-vsock` open for reading and writing. Firecracker
and Cloud Hypervisor need only KVM, because their host end is a Unix
socket. `vhost_vsock_usable` in `xtask/src/vmm.rs` uses the device when it
already opens. It widens the device's mode only when `CI` is set, on a
GitHub runner that is thrown away after the job. There, with password-less
`sudo -n`, it runs `modprobe vhost_vsock`, `udevadm settle` and
`chmod 0666 /dev/vhost-vsock`, in that order: loading the module on the
first open made udev put `0660` back between the `chmod` and QEMU's open.
On any other host the mode is never changed, and the control channel
stays on the serial line until the user is given access (for example,
membership in the device's group).

The mode decides who may *be* a vhost-vsock VMM. Such a user can claim a
CID (taking the one `xtask` would use stops its VM from starting) and,
from inside their own guest, connect to the host's vsock listeners. It
does not decide who may connect *to* a guest: on Linux that needs no
privilege at all. Since S28 such a connection is served only once it
presents the secret the kernel gave on its serial line.

### Findings

**S27. A connection that replaced another between two looks was not
greeted (Low, both).** The control kernel told connections apart only by
the stream's connected flag changing, and it dropped the `Change` that
`Stream::take` returns. Sometimes a reset of the host's connection and a
new request are taken in the same poll: a restarted server or a retrying
client, while a cell runs. The stream is then connected before and after,
so the kernel never saw a new connection:

- the new connection got no hello, which the `control-plane` spec
  requires;
- the decoder kept the old connection's state: half a frame left by the
  old connection was completed with the new connection's first bytes.
  Frames carry no checksum, so the result can decode as a request nobody
  sent.

The host is trusted and no tool reconnects today, so this is a
correctness bug, not an escalation. Found by reading `pump` and
`Server::read`. Reproduced on the host through the simulated device: the
old stream stayed connected, and the old reader got the new connection's
bytes. *Fix* (`fix-vsock-reconnect`,
[PR #142](https://github.com/davidsalvat/rhizo/pull/142)): every
connection has a number, `Stream::read` takes the number of the
connection it reads for, and the control kernel greets and starts a fresh
decoder whenever the number changes. *Test:*
`a_reconnect_between_two_polls_is_a_new_connection`.

**S28. Any local user could take the control channel on QEMU (Low,
both; open finding O8 until `authenticate-control-channel`).** On QEMU
the host connects with an `AF_VSOCK` socket to `(guest CID, 1024)`. Linux
lets any local process open such a connection, without privilege,
whatever the mode of `/dev/vhost-vsock`. The CID follows from the process
identifier of `xtask` or `rhizo`, which `ps` shows. The kernel held one
connection at a time and took the first request it received while it
held none:

- **Before the host connects.** The kernel listens from `vsock::init`,
  before its serial hello, and the host connects only after that hello.
  A local user who kept connecting to the expected CID got there first
  and received the hello. The host got a reset, retried for 10 s, gave up
  and stopped the VM. For those seconds the intruder controlled an empty
  control kernel: they could start cells from its images, have it load a
  snapshot (B3 input in any case), or power it off.
- **After the host's connection ends** while the VM still runs (for
  example, the `rhizo` server was killed and QEMU outlived it). The
  intruder then controlled every cell: they could read output, save
  snapshots of the cells' memory, kill, and power off.

Firecracker and Cloud Hypervisor were not reachable this way. Their host
end is the VMM's Unix socket `$TMPDIR/rhizo-vsock-<cid>.sock`, which only
users with write permission on it can connect to (its mode follows the
VMM's umask).

This is outside the model's main attackers (local host users are
attacker 5 of the audit scope), but snapshot transfers already defend
against other local processes with one-time tokens
(`snapshots-on-disk`), and the serial line that vsock replaced reached no
other user. Found by reading the host ends while documenting
`control-over-vsock`.

*Fix* (`authenticate-control-channel`):

- At boot the control kernel draws a 256-bit secret from its generator
  (seeded from the CPU's random instruction) and prints it on its serial
  line, before the hello, as `vsock: secret <64 hex digits>`. Only the
  process that started the VMM reads that pipe; the secret is never on
  the command line (`/proc/<pid>/cmdline`). Without a hardware random
  source the kernel does not listen on vsock at all, and the frames stay
  on the serial line.
- `rhizo_virtio::vsock::Gate` serves a connection only once its first 32
  bytes are the secret, compared in constant time and only when all 32
  have come (no byte-by-byte answer). Until then it is not greeted, none
  of its bytes is decoded, and nothing is written to it. A wrong secret,
  no secret within 2 s, or another peer's request resets it; the kernel
  keeps listening, counts the refusal, prints it and audits it (an
  `auth-refused` record, folded per second; see
  [The audit log](#the-audit-log-design-d18-of-add-cell-hierarchy)). Every new
  connection, after a drop too, presents the secret again.
- `xtask`'s `Vm`, and so `rhizo up`'s relay, takes the secret line out of
  the console text (it reaches neither `console.log` nor an error
  report), keeps the secret in memory only, and writes it first on every
  vsock connection, on QEMU, Firecracker and Cloud Hypervisor alike.
  `rhizo up`'s state directory is `0700` and its control socket `0600`.
- The frames and the protocol version are unchanged: the secret comes
  before the first frame.

*Tests:* in `rhizo-virtio`, `a_connection_is_admitted_only_with_the_secret`,
`a_wrong_secret_is_refused_and_the_gate_keeps_listening`,
`requests_without_the_secret_never_reach_the_reader`,
`a_slow_prover_is_reset`, `a_secret_split_over_many_packets_is_admitted`,
`a_reconnect_after_a_drop_must_present_the_secret_again`,
`a_stranger_cannot_hold_the_only_connection`, and the properties
`only_the_secret_admits` (arbitrary first bytes, cut into arbitrary
packets) and `hostile_packets_are_never_admitted`; in `xtask`,
`the_secret_line_never_reaches_the_console_text` and
`a_vsock_connection_presents_the_secret_first`; in `rhizo-cli`,
`the_state_directory_is_private`. The `control` test step, on a VM with a
socket device, connects three strangers (frames, a wrong secret followed
by a power off, half the secret then silence), checks that each is reset
without a byte back and that the VM still answers the host, and that the
kernel reported three refusals and sent `auth-refused` audit records
counting three.

*What is left:* a local user can still keep the host out, by claiming
the guest's CID before QEMU starts or by sending requests that keep
replacing the host's connection before its secret arrives. Both are
denial of service with no access to a cell; `xtask` reports the failed
hello and stops the VM.

### What is tested, and what is not

Tested on the host, at the fixed seed of `cargo xtask check`
(`cargo test -p rhizo-virtio -- vsock::`):

- `Stream` alone, by property tests:
  - `hostile_packets_are_survived`: up to 60 steps between writes and
    reads, each an arbitrary header (CIDs and ports drawn from the
    stream's own, the host's or any value; any operation and socket type;
    any credit) with a stated length that may or may not match its
    payload. Checks: no panic, at most 256 KiB unread, what is sent stays
    bounded, and every packet sent carries a payload of its stated length.
  - `credit_is_never_exceeded`: credit updates with any `buf_alloc` and
    `fwd_cnt`.
- `Stream` alone, a unit test for each rule: one connection, resets to
  strangers, a peer that breaks the protocol, unknown operations, credit
  requests, the 4 MiB limit, and bytes that stay "sending" until they are
  out (`written_bytes_are_sending_until_they_go_out`; the power off waits
  for them).
- The driver against a simulated virtio-mmio device that reads and writes
  the rings in fake physical memory: probe, bytes both ways, slot reuse,
  and `a_hostile_device_cannot_overrun_buffers`.

Not covered:

- **The driver against a hostile device** is tested with chosen cases (a
  short buffer, a length past the buffer, two bad descriptor numbers), not
  by a property test. Arbitrary used-ring sequences are not fuzzed: an
  index that jumps, entries out of order or repeated, a receive slot the
  device does not hold. Reading the code finds no out-of-bounds path,
  since every index is folded into the ring. A device that keeps
  advancing the used index does keep the control loop receiving for as
  long as it does so (the device is trusted, B6).
- **The kernel's glue** (`kernel/src/vsock.rs`, the `link` module) runs
  only in the `control` test step: over vsock on CI's runners and on Cloud
  Hypervisor, over the serial line elsewhere. It is not fuzzed; the step
  connects strangers without the secret, but the host never reconnects.
- **The host ends** (`xtask`'s `connect_unix` handshake with Firecracker
  and Cloud Hypervisor, and `connect_vhost`) are host tooling, out of
  scope except for S28.
- **Transport resets** are never received: the event queue is set up with
  no buffer. A VMM sends one after it restores a VM snapshot, and VMM
  snapshots of a control kernel are not supported.

## The audit log (design D18 of `add-cell-hierarchy`)

The kernel records what it refused and what the host administered in a
ring of 64 KiB that no cell can read; `rhizo` appends the records to
`<state>/audit/` as JSON lines, each carrying the SHA-256 of the line
before, and `audit/head` names the last line's hash.

**What `rhizo audit verify` proves.** That the log is the one the server
wrote, unless someone rewrote it from some line to its end: an edited,
removed or reordered line in the middle, a removed file, or a cut end
breaks the chain or disagrees with `head`, and `verify` names where.
Within one boot it also reports records that never reached the log (a gap
in the kernel's sequence numbers) apart from those the kernel dropped
when its ring was full (the drop count the next record carries).

**What it does not prove.** The chain has no key. Anyone who can write the
state directory (root, or the user who runs `rhizo`) can rewrite the log
from an edited line on, recompute every hash and `head`, and `verify`
passes; deleting the whole directory removes the log. The kernel's
sequence numbers and drop counts make records missing from a live
session visible only if the rest of that session was not rewritten to
match.

**Mitigation, not built.** Forward the files as they are written
(journald, syslog, a SIEM), and export `head` to an append-only sink
outside the host's reach, or sign it with a key kept outside the state
directory. Then a rewrite of the chain no longer matches the exported
hash.

## Known limitations and open findings

These are not fixed by this change; each needs its own design.

| # | Severity | Item |
|---|---|---|
| O1 | — | **Closed by `add-cell-quotas`** (see *Quotas*). What remains: a cell without a quota may still take all memory down to the reserve, as before; file data (up to `DATA_MAX`) and names are bounded per cell but not by the memory quota, so a cell holds at most its memory quota plus `DATA_MAX` of file data; a cell may hold its quota plus what it inherited from a fork. |
| O2 | Medium | **Kernel time proportional to what a cell holds.** A call runs without preemption: reading or writing up to 64 MiB of file data, `getrandom` or `write` over gigabytes of the cell's own memory, or mapping and zeroing a large region delays every other cell for as long as it takes. Bounded by the cell's own memory, not by a time slice. |
| O3 | Medium | **Side channels other than Meltdown are out of scope.** Meltdown is closed by `add-kpti` (KPTI on x86_64 where needed, E0PD on AArch64; see *Speculation*). Caches, branch predictors, other speculation attacks (Spectre, L1TF, MDS) and timing are not reviewed. An AArch64 CPU with neither E0PD nor CSV3 is reported, not protected. Cells see a nanosecond clock. |
| O4 | — | **Closed by `add-crypto-random`** (see *Randomness*). What remains: a CPU without a hardware random source (an AArch64 host without FEAT_RNG under KVM) falls back to the cycle counter, and the boot says `getrandom is predictable`; there is no virtio-rng driver yet. |
| O5 | Low | **SMEP, SMAP and PAN closed by `add-user-access-guards`.** Left: SMAP and PAN cannot see accesses through the direct map (software checks guard it); on x86_64, NMI and machine-check have no stack of their own (IST): one arriving in the two instructions of the `SYSCALL` entry before the stack switch would be pushed onto the cell's stack (the platform sends neither today). |
| O6 | — | **Closed by `add-cell-hierarchy`** (phase 1, see [Colluding cells](#colluding-cells-add-cell-hierarchy-phase-1)). Identifiers are numbered from 1 within each domain, and a copy's differs from its source's: nothing another domain does shows in them (`identifiers_are_per_domain`, `differential_colluders`). |
| O9 | Low | **Memory the kernel takes unmetered** (`add-cell-hierarchy` phase 1). Copy-on-write copies of shared frames, the pages a domain keeps in the page cache, pipe buffers and the room of a channel the host makes, or one that outlives the cell that made it, are charged to no limit and come from the memory no reservation holds, so a reserved cell's first write to a page shared with its source, or its write into a pipe, can be refused once other domains have exhausted that memory (cells without a limit, and the unmetered pages of a reserved domain): a fault or an `ENOBUFS` another domain causes. Metering them is left to a later task. The room of a channel a cell makes through a cell handle is metered since task 2.5: counted against the cell's memory before the channel is made (`ENOMEM` when its limit cannot hold it) and given back when the channel ends. |
| O7 | Low | **Host paths may panic.** Forking one cell, or spawning one kept image, more than 65535 times while every copy lives panics (frame reference counts are 16 bits; a cell's own path, mapping file pages, gives copies instead since S14); a frame lost when loading an executable fails half way. Only the host can do either. |
| O8 | — | **Closed by `authenticate-control-channel`** (finding S28, see [The control channel over vsock](#the-control-channel-over-vsock-b7-b8)). What remains: a local user can keep the host from connecting (claiming the guest's CID, or replacing its connection before the secret arrives), without access to any cell; a CPU without a hardware random source keeps the control channel on the serial line. |
| — | Low | **Hosts on the VM's network segment** (B5, above) can evict the card's neighbour-cache entries, delaying a connection's frames by about a second, and a rogue DHCP server can change or take away the card's address. Both come with sharing a segment; give the VM a network of its own. |
| — | — | **Concurrency between CPUs** was reviewed by reading and by hostile cells (second review), not by a model checker or a proof. |
| — | — | **No hardware, firmware or VMM audit.** Device emulation, the boot path and the VMMs are trusted. |
| — | — | **No formal proof.** This is a review with tests, not a verification. |

## Tests that keep it fixed

The second review's fuzzers and hostile cells, and the network fuzzer of
the review of B5, are listed in their own sections above. From the first
review:

- Host, `crates/rhizo-abi/src/hostile.rs`: `x86_64_cells_cannot_hurt_the_kernel`
  and `aarch64_cells_cannot_hurt_the_kernel` (properties: up to 40 calls of
  any number and arguments; results are values or errnos, no panic, kernel
  tables, data and code unchanged, console reports bounded, every frame
  back), `pointers_outside_the_cell_give_efault` (property, 27 calls taking
  a pointer), and one test per finding.
- KPTI: `kpti_leaves_cells_only_the_entry_pages` (x86_64),
  `e0pd_is_on_where_present` (AArch64), `cells_run_alike_with_and_without_kpti`;
  every other in-kernel test runs with KPTI on.
- Host, elsewhere: `ranges_outside_the_user_half_are_refused` (property,
  `rhizo-cell`), `the_kernel_half_is_out_of_reach` (`space`, both formats),
  `hostile_headers_never_panic` (ELF), `kernel_half_is_never_made_writable_or_private`
  and `holes_end_where_their_empty_entry_ends` (`rhizo-mm`),
  `files_and_directories_are_limited_per_cell` (`rhizo-fs`), the snapshot
  register tests.
- In the kernel, both architectures (`kernel/src/tests/isolation.rs`),
  with hostile cells in `cells/src/bin`:
  - `syscall_fuzzer_cannot_hurt_the_kernel`: `fuzz`, 20000 calls from a
    fixed seed next to a well-behaved cell; nothing leaks.
  - `hostile_pointers_get_efault`: `hostile_ptrs` (also run on Linux by the
    differential check, which answers the same).
  - `cell_cannot_read_kernel_memory` (`fault_kernel`, existing) and
    `cell_cannot_write_kernel_memory` (`poke_kernel`).
  - `memory_hog_is_refused_and_others_run`: `hog`.
  - `syscall_storm_does_not_starve_others`: `storm`; and
    `spinning_cell_does_not_starve_others` (`spin`, existing) for a cell
    that never makes a call.
  - `fresh_memory_never_holds_another_cells_data`: `scrub`, after a cell
    filled frames with a pattern: a fresh cell, a forked child, eager and
    lazy resumes all read zeros; the ended cell's mapping faults.
  - `names_cannot_fill_kernel_memory`: `namer` cells, with and without a
    cell taking all memory.
  - `shared_read_only_pages_stay_shared_after_mprotect`: `rewrite`, forked,
    each copy makes its constants writable and writes them.
  - Quotas: `memory_quota_refuses_the_cell_not_the_others` (`hog` with a
    quota next to `ballast`), `cpu_quota_ends_the_cell` (`spin`),
    `file_and_socket_quotas_travel_with_the_cell` (`limits`, forked,
    copied from a template, resumed), `a_cell_holds_a_bounded_share_of_sockets`
    (S21); `hog file` for S20; host tests in `rhizo_cell::quota`,
    `quotas_limit_descriptors_and_sockets` (`rhizo-fs`),
    `rlimits_report_the_cells_quotas` (`rhizo-abi`).
  - `flags_a_cell_sets_stay_in_the_cell`: `flags`; `user_guards_are_on`
    (x86_64), `pan_is_on` (AArch64); self-checks `ktest-user-read` and
    `ktest-user-exec`.
  - `no_two_cells_draw_the_same_random_bytes`: `dice`, forked, copied from
    a template and resumed twice from one snapshot; and
    `hardware_entropy_works_where_present`.
- Cross-cell leaks (S24 to S26), in the kernel with `observer` and `hog`:
  `sysinfo_shows_a_cell_only_itself`,
  `ephemeral_ports_do_not_show_other_cells_binds`,
  `memory_pressure_keeps_the_pages_other_cells_map`; on the host the tests
  named under each finding.
- Colluding cells (`add-cell-hierarchy` phase 1, S29, S30), in the kernel
  with `colluder` (`kernel/src/tests/collusion.rs`):
  `differential_colluders`, `reserved_probe_beside_an_unlimited_hog`,
  `colluders_on_loopback`,
  `fork_bomb_in_a_subtree`, `forged_snapshot_authority`; with the tests
  named in the inventory's last column.
- The control channel over vsock (S27, S28), on the host in
  `rhizo-virtio`: `hostile_packets_are_survived`,
  `credit_is_never_exceeded`, `only_the_secret_admits`,
  `hostile_packets_are_never_admitted` (properties), the gate's tests
  under S28,
  `a_hostile_device_cannot_overrun_buffers`,
  `a_peer_that_never_reads_cannot_fill_memory`,
  `a_reconnect_between_two_polls_is_a_new_connection`.
- Host, `rhizo-core`: `block_function_matches_rfc_8439`,
  `the_key_is_erased_after_every_request`, `split_generators_share_no_output`.
