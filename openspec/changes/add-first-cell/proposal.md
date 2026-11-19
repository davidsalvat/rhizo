# Proposal

## Why

Rhizo's unit is the agent cell, and so far there is none: the kernel only
runs its own code. H2 creates the first cell: an isolated address space in
user mode that runs an unmodified static Linux program and talks to the
kernel through the Linux system-call ABI. Everything after (scheduling,
suspend/resume, fork) builds on this.

## What Changes

- New crate `rhizo-cell`: the cell model (identifier, address space, user
  registers, program break, mapping regions) designed as plain serializable
  data, and a loader for static 64-bit ELF executables (segments, `.bss`,
  stack with `argv`, `envp` and the auxiliary vector), host-tested.
- New crate `rhizo-abi`: the Linux system-call layer (dispatch by
  architecture-specific numbers, a first set of calls, `ENOSYS` with a report
  for anything else), host-tested against a fake cell.
- Architecture code to enter user mode and come back on system calls and
  faults: EL0 with `SVC` on AArch64, ring 3 with `SYSCALL` on x86_64.
- A cell fault (bad pointer, illegal instruction) ends the cell with a report;
  the kernel keeps running.
- Test programs: small static Linux executables, built by `xtask` without a
  libc, that run unchanged on Linux too (used as a differential check).
- First cell benchmarks: `cell.spawn_us` (load, run to exit) and
  `cell.pt_frames`; anonymous mappings of 2 MiB or more use 2 MiB pages.

### Out of scope

Several cells and scheduling (H3), interrupts and preemption, files and a
file system beyond the console, signals beyond accepting their setup calls,
threads, dynamic linking and position-independent executables, real libc
binaries (H6), multicore.

## Capabilities

### New Capabilities

- `cells`: creating a cell from an executable, its address space and
  isolation, its exit and its faults.
- `linux-abi`: which Linux system calls a cell can use and how unsupported
  ones behave.

### Modified Capabilities

- `cpu-exceptions`: faults raised by cell code end the cell instead of the
  VM; the separate exception stack keeps working with user mode.

## Impact

- New crates `rhizo-cell` and `rhizo-abi` (added to the dependency table).
- Kernel: user-mode entry and system-call paths in both `arch/` backends,
  GDT user segments and syscall MSRs on x86_64.
- `xtask`: builds the test programs, new self-check (cell fault does not stop
  the kernel), new benchmark metrics.
