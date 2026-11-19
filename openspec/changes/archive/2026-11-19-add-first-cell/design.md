# Design

## Context

See `proposal.md`. After H1.6, AArch64 runs the kernel on `SP_EL0` and takes
exceptions on `SP_EL1` (so stack overflows are reported); x86_64 uses an IST
stack for double faults. Neither has user mode, a TSS kernel stack for ring
transitions, or a system-call entry.

## Goals / Non-Goals

**Goals:** one cell runs a static Linux program to completion on both
architectures; the cell's state is plain data (H4 snapshots it); every
decision measured where it touches speed.

**Non-Goals:** fairness, preemption, interrupts (H3); a VFS.

## Decisions

### D1. Crates

- `rhizo-cell`: `Cell` (id, root page table, `Registers`, program break,
  mapping cursor, exit status) and `elf` (validate and plan the loading of
  static `ET_EXEC` images: segments, permissions, initial stack). It produces
  a plan and writes through `PhysicalMemory`, so it is tested on the host.
- `rhizo-abi`: `dispatch(arch, number, args, &mut impl CellContext) ->
  Outcome` where `CellContext` gives the handler what it may touch (copy from
  and to cell memory, console, clock, map/unmap). Host tests use a fake
  context.

Dependency rules: `rhizo-cell` → `hal`, `mm`; `rhizo-abi` → `hal`, `mm`,
`cell`.

### D2. Register state is architecture-neutral data

`Registers { pc, sp, args: [u64; 6], ret, tls, flags, gpr: [u64; 31] }` holds
everything needed to resume a cell on either architecture; the `arch` layer
converts to and from its trap frame. Being plain data, it is what H4
serializes and H5 copies.

### D3. AArch64: which stack the kernel runs on — options

| Option | How | For | Against |
|---|---|---|---|
| A. Kernel always on `SP_EL1`, Linux-style overflow check in the vector entry with a small overflow stack | Every exception, kernel or user, runs on `SP_EL1` | Same scheme as Linux | Changes the H1.6 stack-overflow mechanism; the check costs instructions on every entry |
| **B. Kernel on `SP_EL0`, `SP_EL1` as entry stack (recommended)** | Entry from EL0: vector runs on `SP_EL1`, saves the user frame there, sets `SP_EL0` to the kernel stack and switches to `SPSel = 0`; return restores the user `SP_EL0` | Keeps the H1.6 invariant and its overflow report unchanged; two `msr` per transition | User `SP_EL0` must be saved/restored explicitly (it is part of the frame anyway) |

### D4. x86_64 user mode

GDT gains user code/data segments in the order `SYSRET` requires; `STAR`,
`LSTAR`, `SFMASK` are programmed; `EFER.SCE` set. The TSS `RSP0` points at
the kernel stack for faults from ring 3. The `SYSCALL` entry switches to the
kernel stack from a per-CPU slot (single CPU: a static), saves the frame,
calls Rust, and returns with `SYSRET` (with the canonical-RIP check).
`FS` base is set by `arch_prctl(ARCH_SET_FS)`.

### D5. Cell address space

Each cell gets its own root table. The kernel half is shared: on AArch64 it
lives in `TTBR1` and the cell's table goes in `TTBR0`; on x86_64 the cell's
root copies the kernel's upper-half entries (256–511). Cell pages are
`USER`, W^X is enforced (a segment both writable and executable is
rejected). Layout: image at its link address, stack top at
`0x0000_7fff_ffff_f000` (8 MiB reserved, guard page below), anonymous
mappings from `0x0000_4000_0000_0000` upwards, 2 MiB-aligned and mapped with
2 MiB pages when the request is at least 2 MiB.

### D6. System calls in H2

`write`/`writev` (fd 1 and 2 to the console), `exit`, `exit_group`, `brk`,
`mmap` (anonymous, private), `munmap`, `getpid`, `gettid`, `set_tid_address`,
`uname`, `clock_gettime` (monotonic and realtime from the counter),
`getrandom`, `rt_sigaction` and `rt_sigprocmask` (accepted, no effect),
`ioctl` (`ENOTTY`), `arch_prctl` (x86_64 TLS). Anything else returns
`-ENOSYS` and prints `cell N: unimplemented syscall <name|number>` once, so a
missing call is visible and actionable.

`getrandom` uses a ChaCha-free, deterministic-per-boot generator seeded from
the counter; determinism for snapshots is H4's problem and noted there.

### D7. Faults inside a cell

An exception from user mode (data abort / page fault, undefined
instruction) prints `cell N killed: <report>` with the same decoding as kernel
faults, frees the cell, and returns to the kernel, which continues.

### D8. Test programs

`cells/` holds tiny `#![no_std]` Rust programs built for the bare-metal
targets with Linux syscall numbers; they are valid static Linux executables.
`xtask` builds them before the kernel and passes their paths to the kernel
build (`include_bytes!`). The same binaries run on a Linux host of the same
architecture, which gives the differential check (`xtask` compares
outputs where the host matches).

### D9. Benchmarks

A `bench-cell` kernel feature spawns the hello cell N times and prints
`rhizo-cell spawn_ns=<median>`; `xtask bench` records `cell.spawn_us` and
`cell.pt_frames` per VMM.

## Risks / Trade-offs

- [SYSRET with a non-canonical RIP is a known x86 hazard] → validate RIP
  before `SYSRET`, else return through `IRETQ`.
- [Two user-mode entries to keep in sync] → behavior tests are shared; the
  same cell programs run on both architectures.
- [Fake randomness] → documented, not security-relevant in H2.
