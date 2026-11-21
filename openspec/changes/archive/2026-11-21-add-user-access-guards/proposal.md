# Proposal

## Why

The kernel never dereferences a cell's address: every copy translates it
in the cell's table and goes through the direct map. Nothing in the CPU
enforces that, though (open item O5 of `docs/security.md`): a kernel bug
that followed a cell pointer, or jumped to one, would read, write or run
whatever the cell put there. x86_64 has SMEP and SMAP for this, AArch64
PAN (and `PXN`, already set on every cell page).

Reading the entry code for this change also found a real bug: on x86_64
an exception or interrupt from ring 3 keeps the cell's `RFLAGS.DF` and
`RFLAGS.AC` (an interrupt gate clears only IF and TF). Rust code then ran
with the direction flag a cell chose, which the ABI forbids (string
instructions in `memcpy` would copy backwards), and with SMAP on, a cell
setting AC would have lifted SMAP for the kernel.

## What Changes

- x86_64: CR4.SMEP and CR4.SMAP on every CPU that has them; the
  exception entry from ring 3 loads RFLAGS with only its reserved bit
  (clears DF and AC) before any other instruction. `SYSCALL` already
  clears both through `SFMASK`.
- AArch64: `PSTATE.PAN` on every CPU that has it, with `SCTLR_EL1.SPAN`
  cleared so every exception taken to EL1 sets it again.
- No user-access helper needs `stac`/`clac` or a PAN toggle: the kernel's
  only way to cell memory is the direct map, which these guards do not
  cover (its pages are supervisor pages). The direct map is guarded by the
  software checks of `rhizo_cell::memory` (the cell's own table, user and
  write bits, `USER_END`), as before.
- Tests: two self-checks (`ktest-user-read`, `ktest-user-exec`) in which
  the kernel reads, then calls, a user page and must fault; an arch test
  that the guards are on; and a cell (`flags`) that spins with DF and AC
  set next to other cells, with a debug assertion on every return from a
  cell that the kernel's flags are clean.

### Out of scope

The direct map itself (SMAP cannot see through it); separate stacks for
NMI and machine check (the platform sends neither); side channels (KPTI
is its own change).

## Capabilities

### Modified Capabilities

- `isolation`: the CPU refuses kernel accesses to cell pages, and a
  cell's flags never reach kernel code.

## Impact

`kernel/src/arch/x86_64/{cpu,user,tests}.rs`,
`kernel/src/arch/aarch64/{user,tests}.rs`, `kernel/src/tests.rs`,
`kernel/Cargo.toml` and `xtask/src/main.rs` (self-checks),
`cells/src/bin/flags.rs`, `cells/expected.rs`, `docs/security.md`.
No cost on the system-call path (no `stac`/`clac`); one `popfq` on each
exception or interrupt from a cell.
