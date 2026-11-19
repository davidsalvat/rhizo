# Design

## Decisions

### D1. FP/SIMD state: switched on first use

The kernel is built without FP/SIMD, so the registers always hold the state
of the cell that last used them (the owner). A cell that is not the owner
runs with FP/SIMD disabled for it (`CPACR_EL1.FPEN = 0b01`, which traps EL0
only; `CR0.TS` on x86_64); its first FP/SIMD instruction traps, and only
then is the owner's state saved and the cell's loaded. The owner is synced
before a suspend, fork or freeze.

| Option | For | Against |
|---|---|---|
| **Trap on first use (chosen)** | Cells that never use FP never pay; an FP cell pays once per switch-in | A trap path per architecture |
| Swap whenever a different cell runs (first version) | No trap path | Measured in CI with KVM: `cell.switch_ns` 303 → 572 ns, `cell.run_us` 7.2 → 12.4 µs, for cells that never touch FP |
| Save/restore on every entry and exit | Simplest | ~1 KB copied twice per system call |

State layout: 66 words (`V0`–`V31`, `FPCR`, `FPSR` / the `FXSAVE` image),
16-byte aligned, at a fixed offset in `Registers`. Initial state as Linux:
zero, with `FCW = 0x37f` and `MXCSR = 0x1f80` on x86_64.

### D2. Real programs from the system toolchain

C test programs are compiled with the distribution's gcc and glibc
(`gcc`/`libc6-dev`, `gcc-aarch64-linux-gnu` for cross builds), statically.
They are the same kind of binary people ship, need no downloaded toolchain,
and run natively on a Linux host of the same architecture for the
differential check.

### D3. Linux behavior for missing facilities

Where Rhizo lacks a facility, calls answer as Linux does on a system
without it: `readlink` finds no link (`ENOENT`), the standard descriptors
are character devices, limits are the usual defaults, `rseq` is not
available. Anything else still returns `ENOSYS` and is reported.
