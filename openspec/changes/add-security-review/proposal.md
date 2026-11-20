# Proposal

## Why

The thesis is "isolated and fast". Rhizo measures speed in every pull
request but nothing checks isolation against someone trying to break it.
Cells will run untrusted agent code: the kernel must assume the code in a
cell is hostile and makes any system call with any arguments. Without an
adversarial review, half of the thesis is unproven.

## What Changes

- A review of the boundary between a cell and the kernel, and between
  cells (`docs/security.md`): threat model, trust boundary, every system
  call with what it takes from the cell and how it is checked, findings
  with their fix, and what is out of scope.
- Fixes for what the review found:
  - `mprotect` no longer reaches the kernel half (on x86_64 a cell could
    make kernel memory its own, and read and write all of RAM).
  - `mprotect` no longer makes a page shared with a fork writable in place
    (a cell could write its parent's, siblings' or template's code); it
    becomes copy-on-write.
  - A write fault on a kernel address is never taken for copy-on-write
    (on x86_64 such a cell was never killed); copy-on-write walks stay in
    the lower half.
  - `arch_prctl(ARCH_SET_FS)` refuses a kernel or non-canonical base
    (`EPERM`, as Linux), which would fault in the kernel when loaded.
  - `munmap` and `mprotect` skip empty space a table entry at a time
    instead of a page at a time (one call could stop every cell for hours).
  - Names are bounded: at most 4096 files and directories per cell, and
    creating one asks for memory first (`ENOSPC`), like file data; a
    `getdents64` answer is at most 64 KiB; captured output at most 1 MiB.
  - `mmap` and `brk` refuse memory that cannot be had before zeroing it.
  - Cells end at `0x7fff_ffff_f000`, as on Linux: no executable page where
    a `syscall` would return to a non-canonical address.
  - Snapshots whose registers would fault in the kernel are refused.
  - Unknown call numbers past every Linux one share one report.
  - ELF header arithmetic cannot overflow.
- Lasting hostile tests: host property tests that drive the dispatcher
  with any number and arguments against a kernel half and check kernel
  memory never changes; in-kernel tests with hostile cells (a syscall
  fuzzer, kernel pointers, writes to kernel memory, memory and name
  exhaustion, a call storm, fresh memory after spawn, fork and resume).

### Out of scope

Side channels (caches, speculation, timing), multicore, quotas set by the
host, hardware and hypervisor review, cryptographic randomness. They are
listed in `docs/security.md` as known limitations.

## Capabilities

### New Capabilities

- `isolation`: what a hostile cell cannot do to the kernel or to other
  cells, and the bounds the kernel keeps per call and per cell.

### Modified Capabilities

- `linux-abi`: unknown call numbers past every Linux one share one report.

## Impact

`crates/rhizo-abi` (checks, hostile tests), `crates/rhizo-cell` (`USER_END`,
range checks, ELF), `crates/rhizo-mm` (hole skipping, copy-on-write stays
in the lower half), `crates/rhizo-fs` (file limit), `crates/rhizo-snapshot`
(register checks), `kernel/src/cell.rs` (captured output),
`kernel/src/tests.rs`, `cells/src/bin` (hostile cells), `docs/security.md`.
No benchmarked path gets slower: the new checks are compares, and the
range walks get faster.
