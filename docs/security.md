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
S14 to S21, section [Second review](#second-review-d2)).

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

**Second attacker: a hostile snapshot** (from the second review on).
Templates will be downloaded as snapshots (`docs/vision.md`), so the bytes
`Snapshot::from_bytes` reads may be written by anyone: they must be
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
crash the kernel), not against a malicious device.

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
- **Per-cell state.** Registers, FP/SIMD state (switched lazily, trapped on
  first use), files and descriptors (`rhizo_fs::Fs`, private copy-on-write)
  and the system-call handler (`Abi`) belong to one cell.
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

## Speculation (Meltdown)

`add-kpti` closed the Meltdown part of O3.

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
| `prlimit64` (261/302), `getrlimit` (163/97) | resource, old-limit buffer | resource ≤ 15 (`EINVAL`); new limit and pid not read; 16 bytes written |
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
| S20 | Low | both | `mmap` of a file maps holes and pages past its end with fresh zeroed frames without asking memory first (as S11 did for anonymous memory) | Accepted (below) |
| S21 | Medium | both | Sockets are bounded kernel-wide (4096): one cell can hold about half, two all, and every other cell gets `ENFILE` | Accepted until per-cell socket quotas (O1, `add-cell-quotas`) |

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
| **Sockets** (`rhizo-net`, loopback, Unix, UDP, `smoltcp` on the card) | Exhausting sockets, ports, backlogs and buffers shared by every cell; long or malformed addresses, `msghdr` and `iovec`; `ioctl` | Buffers are bounded (256 KiB per stream, `DGRAM_MAX`) and asked from memory before they grow; paths ≤ 107 bytes; `iovec` counts ≤ 1024 and lengths ≤ `SSIZE_MAX`, their sum saturates (S16); `ioctl` takes `FIONBIO`, `FIOCLEX`, `FIONCLEX` only, on the caller's own descriptor. Ephemeral ports and sockets are a kernel-wide pool (S21). A Unix name is a number in the cell's own tree: a cell reaches another's Unix socket only if its files hold the name. |
| **Parked connections** (`keep-tcp-across-suspend`, snapshot v6) | A forged snapshot naming another suspended cell's parked connection | Since S15, keys are SipHash-2-4 of a counter under a boot secret no snapshot holds; a key that matches nothing comes back reset or bound again; each key re-attaches once. |
| **virtio** (mmio, pci) | — (the device is trusted, above) | Robustness only: configuration-space parsing bounded (48 capabilities, offsets inside the 4 KiB function), BARs naturally aligned powers of two below the top of memory (S17), structures inside their BAR. Fuzzed (`any_configuration_space_is_set_up_or_refused`). |
| **Dynamic linking** (`PT_INTERP`, file `mmap`, `MAP_FIXED`, the page cache) | Mapping file pages without end; writing through a private mapping of a page other cells share; `MAP_FIXED` over its own or the kernel's memory; a planted interpreter with hostile headers | The cache holds each page it keeps, so a block's bytes cannot change while a frame shows them (a write gives the file a new block, hence a new key); file frames are mapped read-only, writable mappings copy-on-write (S12 rule); a count that cannot grow gives a private copy (S14); `MAP_FIXED` ends at or below `USER_END` and not on page 0; interpreters must be position independent with no interpreter of their own, and every entry point is checked (S18). Mapped holes and pages past the end are not asked for first (S20). |
| **Image cache and 2 MiB pages** (`add-image-cache`, `add-large-image-pages`) | Writing a read-only page or 2 MiB page shared with every spawn of the image | Copies share read-only pages and copy writable ones; `mprotect` of a shared page (2 MiB included) adds copy-on-write; `mprotect` of part of a 2 MiB page is refused; a 2 MiB copy-on-write fault copies into a 2 MiB frame or, without one, into 512 small pages. |
| **Agent workload host calls** (`Host::write_file`, `Host::asleep`, `ioctl`) | — (host calls) | `write_file` reaches a cell only while no CPU runs it (a running cell is out of the scheduler's lists), and goes through the cell's own file limits; `asleep` only reads. |
| **Write-or-execute pages** (`node-jit`) | A JIT's writable and executable memory: a page writable and executable at once, one cell changing another's code, a range list without bound, a page that flips forever | The page tables never hold both rights (a property test over maps, protects, forks, clones, writes and fetches on both formats); a change of state makes the cell's tables its own first and copies a frame still shared before writing it; each change flushes the old translation from every CPU that may hold it (broadcast on AArch64, the gang's hold and KPTI's stale marks on x86_64) and, on AArch64, cleans and invalidates the caches before the thread runs again; at most 64 ranges per cell; an instruction that writes its own page ends the cell. Cells may maintain caches by address on AArch64 (`SCTLR_EL1.UCT`, `UCI`), as on Linux: only their own readable memory. |
| **Snapshots** (format v8) | Any bytes as a snapshot | `from_bytes` checks header, version, machine, the write-or-execute ranges (at most 64, aligned, sorted, apart, inside the user half), every page record (size, user-only flags, W^X, a write-or-execute page writing or running, alignment, order, inside the user half, data in bounds), every thread (identifier, wait, futex address, program counter and on x86_64 thread pointer and `MXCSR`), the file system (`Fs::decode`: every index, link count, tree shape and bound) and the sockets (each record held by exactly one description). The heap and mapping cursors are not checked: they only steer where the cell's own later calls map, and every mapping call refuses ranges past `USER_END`. Fuzzed with tampered and random snapshots restored into a cell that shares a kernel half (below); no finding. |
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

**S20. File holes mapped without asking (Low, accepted).** A file mapping
gives each hole, and each page past the file's end, a fresh zeroed frame
without asking memory for the whole length first, as S11 made anonymous
mappings do: a call over terabytes zeroes all free memory down to the
kernel's reserve (kept by the frame allocator) before it is refused and
gives everything back. Bounded by the reserve, no panic; accepted, and
noted for per-cell quotas (`add-cell-quotas` must meter these frames,
which the page cache takes from the kernel's frames, not the cell's).

**S21. One pool of sockets (Medium, accepted).** At most 4096 sockets are
open in the kernel. A cell with 1024 descriptors can open as many
connections to its own listener (each also holding the accepted end), so
one cell holds about half the pool and two hold all of it; parked sockets
count too. Others then get `ENFILE`; the kernel survives. Accepted until
per-cell socket quotas (O1, `add-cell-quotas`, which bounds sockets per
cell).

### Fuzzers and hostile cells

| Fuzzer | Input | Checks |
|---|---|---|
| `x86_64_cells_cannot_hurt_the_kernel`, `aarch64_...` (first review) | Up to 40 calls of any number and arguments | Values or errnos, kernel memory and tables unchanged, bounded reports, every frame back |
| `decoded_files_cannot_hurt_the_kernel` | A file system with every kind of object, encoded, up to 3 bytes changed and maybe cut, then up to 30 calls on descriptors and paths | Refused, or the same as above with the decoded files |
| `random_bytes_are_refused_or_work` | Random bytes as an encoded file system, then calls | As above |
| `tampered_snapshots_are_refused_or_contained` | A snapshot with threads, pages (two write-or-execute, writing and running), a write-or-execute range, files, a pipe and two sockets, up to 3 bytes changed (mostly in headers and records) and maybe cut | Refused, or restored into a cell sharing a kernel half: kernel memory and entries unchanged, every page in the user half with user flags and W^X, ranges below `USER_END` and at most 64, registers below `USER_END`, every descriptor answers, sockets import, every frame back |
| `random_bytes_are_refused_or_contained` | Random bytes behind a valid header | As above |
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

## Known limitations and open findings

These are not fixed by this change; each needs its own design.

| # | Severity | Item |
|---|---|---|
| O1 | Medium | **No per-cell quota.** One cell may take all memory down to the kernel's reserve; others are then refused (`ENOMEM`, `ENOSPC`), and the kernel survives (spec `cells`). Host-set quotas need per-cell accounting of frames and heap. |
| O2 | Medium | **Kernel time proportional to what a cell holds.** A call runs without preemption: reading or writing up to 64 MiB of file data, `getrandom` or `write` over gigabytes of the cell's own memory, or mapping and zeroing a large region delays every other cell for as long as it takes. Bounded by the cell's own memory, not by a time slice. |
| O3 | Medium | **Side channels other than Meltdown are out of scope.** Meltdown is closed by `add-kpti` (KPTI on x86_64 where needed, E0PD on AArch64; see *Speculation*). Caches, branch predictors, other speculation attacks (Spectre, L1TF, MDS) and timing are not reviewed. An AArch64 CPU with neither E0PD nor CSV3 is reported, not protected. Cells see a nanosecond clock. |
| O4 | — | **Closed by `add-crypto-random`** (see *Randomness*). What remains: a CPU without a hardware random source (an AArch64 host without FEAT_RNG under KVM) falls back to the cycle counter, and the boot says `getrandom is predictable`; there is no virtio-rng driver yet. |
| O5 | Low | **SMEP, SMAP and PAN closed by `add-user-access-guards`.** Left: SMAP and PAN cannot see accesses through the direct map (software checks guard it); on x86_64, NMI and machine-check have no stack of their own (IST): one arriving in the two instructions of the `SYSCALL` entry before the stack switch would be pushed onto the cell's stack (the platform sends neither today). |
| O6 | Low | **Identifiers are global counters.** A cell's identifier (`getpid`) tells how many cells were created before it. |
| O7 | Low | **Host paths may panic.** Forking one cell, or spawning one kept image, more than 65535 times while every copy lives panics (frame reference counts are 16 bits; a cell's own path, mapping file pages, gives copies instead since S14); a frame lost when loading an executable fails half way. Only the host can do either. |
| S20 | Low | **File holes mapped without asking first** (above): bounded by the kernel's reserve. |
| S21 | Medium | **One kernel-wide pool of 4096 sockets** (above): closed by per-cell socket quotas. |
| — | — | **Concurrency between CPUs** was reviewed by reading and by hostile cells (second review), not by a model checker or a proof. |
| — | — | **No hardware, firmware or VMM audit.** Device emulation, the boot path and the VMMs are trusted. |
| — | — | **No formal proof.** This is a review with tests, not a verification. |

## Tests that keep it fixed

The second review's fuzzers and hostile cells are listed in its own
section above. From the first review:

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
- In the kernel, both architectures (`kernel/src/tests.rs`, H13 section),
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
  - `flags_a_cell_sets_stay_in_the_cell`: `flags`; `user_guards_are_on`
    (x86_64), `pan_is_on` (AArch64); self-checks `ktest-user-read` and
    `ktest-user-exec`.
  - `no_two_cells_draw_the_same_random_bytes`: `dice`, forked, copied from
    a template and resumed twice from one snapshot; and
    `hardware_entropy_works_where_present`.
- Host, `rhizo-core`: `block_function_matches_rfc_8439`,
  `the_key_is_erased_after_every_request`, `split_generators_share_no_output`.
