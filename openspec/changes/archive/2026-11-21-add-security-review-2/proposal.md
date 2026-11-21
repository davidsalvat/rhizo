# Proposal

## Why

`docs/security.md` reviewed the cell boundary of a single-CPU kernel with
files only (milestone H13). Since then the kernel runs threads of one cell
on several CPUs, has sockets and a network card (virtio-mmio and
virtio-pci), runs dynamically linked programs from the cell's files with a
page cache shared by every cell, keeps executables loaded with 2 MiB pages,
parks TCP connections across suspend, and grew host calls for the agent
workload. And snapshots will come from outside: templates are downloads
(`docs/vision.md`). None of that was reviewed against an attacker (D.2).

## What Changes

- The second review in `docs/security.md`: a second attacker (a hostile
  snapshot, and files a cell wrote that the host starts as a program), a
  threat model per new area, findings S14 to S21 with severity and status,
  and what stays open.
- Fuzzers (property tests with fixed seeds, run by `cargo xtask check` on
  every pull request, and weekly with random seeds and 4096 cases):
  system calls on a file system decoded from tampered or random bytes;
  tampered and random snapshots restored into a cell that shares a kernel
  half; tampered programs and interpreters started from a cell's files;
  virtio-pci configuration space with changed dwords and BAR sizes.
- A hostile cell `racer` and an in-kernel test of threads racing on
  several CPUs (copies against `munmap`, a futex word unmapped under its
  waiter, `exit_group` during system calls, `mprotect` under a writer).
- Fixes for the findings these turned up:
  - S16 (Medium): `sendmsg`/`recvmsg` sum buffer lengths without overflow
    (it panicked kernels built with overflow checks).
  - S18 (Medium, x86_64): an executable's or interpreter's entry point at
    or past `USER_END` is refused (`ElfError::BadEntry`).
  - S17 (Low): virtio-pci leaves out a BAR whose size is not a power of
    two or that wraps past the top of memory.
  - S19 (Low): `dup2(fd, fd)` compares and returns 32-bit descriptors.
- S14 (High) and S15 (Medium) are fixed by their own changes,
  `fix-shared-frame-count` and `fix-parked-keys`. S20 (Low) and S21
  (Medium, until per-cell socket quotas) are accepted with their reasons.

### Out of scope

A malicious device or VMM, side channels, coverage-guided fuzzing of the
kernel binary, and a model of the TLB shootdown protocol (all listed in
`docs/security.md`).

## Capabilities

### Modified Capabilities

- `isolation`: state the kernel loads stays the cell's (entry points), and
  snapshots from outside are contained.

## Impact

- `crates/rhizo-abi` (`sockets`, `dup2`, fuzzers), `crates/rhizo-cell`
  (entry points), `crates/rhizo-virtio` (BARs), `crates/rhizo-snapshot`
  (fuzzers), `cells/src/bin/racer.rs`, `kernel/src/tests.rs`,
  `docs/security.md`, the weekly workflow.
- No benchmarked path changes: one compare per spawn, a saturating add per
  `sendmsg`/`recvmsg`. Host tests take about 10 s longer.
