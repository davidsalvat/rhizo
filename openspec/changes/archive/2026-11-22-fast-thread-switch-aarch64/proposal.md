# Proposal

## Why

Since #139 (one loopback network per cell), `cell.loopback_rtt_insns`
measures two threads of one cell talking over TCP on 127.0.0.1. On
AArch64 it rose to 12,000 instructions, against 9,206 for the old
two-cell version; x86_64 counts 8,674.

The #139 investigation found the cause: a switch between threads of one
cell moves whole thread records (about 860 bytes, FP/SIMD state
included) several times, and on AArch64 every move is `compiler_builtins`'
`memcpy`, a loop of 8-byte loads and stores (4 instructions per 8 bytes,
22 when source and destination are aligned differently). A table where
records never move brought loopback to about 7,730, but its branch on
every current-thread access failed two x86_64 gates
(`cell.syscall_insns`, `cell.pipe_rtt_insns`).

The same `memcpy`, `memset` and a byte-loop `memcmp` serve every other
copy the AArch64 kernel makes: snapshot encoding, suspend, resume,
fork's records, socket buffers, file blocks.

## What Changes

- **The kernel's own `memcpy`, `memmove`, `memset` and `memcmp` on
  AArch64**, in `kernel/src/arch/aarch64/mem.rs`: strong symbols that
  replace `compiler_builtins`' weak ones. 64 bytes per loop in `LDP`/`STP`
  pairs of general registers, lengths below 64 by size class with
  overlapping accesses at both ends, `memmove` forward or backward by
  overlap, `memcmp` 16 bytes per loop. No SIMD register (the kernel stays
  soft-float; a cell's vector registers are live in the CPU). x86_64 keeps
  `compiler_builtins`' `rep movs`/`rep stos` routines.
- **A thread switch inside a cell trades two records instead of moving
  them out and back**: `Threads::next` copies the next thread into the
  current slot and the previous one into its place (three copies through
  `memcpy` instead of four and a `memmove`); with two threads nothing
  else moves. The current thread stays inline: its accessors do not
  change, so system calls pay nothing.
- **Tests**: an in-kernel test checks the four routines against a byte
  model for every length 0–160 at every pair of offsets modulo 16,
  overlapping moves at every distance up to 72 bytes in both directions,
  whole pages, and random lengths and offsets up to 4,096, on both
  architectures.

### Out of scope

- A table where thread records never move (indices or pointers to the
  current thread): every current-thread access pays an indirection or a
  branch, and the turn order between threads would change (design D3).
- Copying only live FP/SIMD state, or moving it out of the record
  (`rhizo-snapshot` and the ABI read it in place).
- `DC ZVA` in `memset`, FEAT_MOPS, SIMD copies.
- Lowering `bench/baseline.json` values (a later archive round).

## Capabilities

- `physical-memory`: copies that are not whole pages go through the
  kernel's own `memcpy` family on AArch64.
- `threads`: a switch between threads of a cell copies each record at
  most once.

## Impact

- `kernel/src/arch/aarch64/mem.rs`, `kernel/src/arch/mod.rs` (interface
  list), `kernel/src/tests/mm.rs` (test).
- `crates/rhizo-cell/src/threads.rs`: `Threads::next`.
- No benchmark is added: `cell.loopback_rtt_insns` shows the switch, and
  the suspend, resume, snapshot, fork and file figures show the routines.
