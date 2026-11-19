# Design

## Decisions

### D1. FP/SIMD state: lazy switching

The kernel is built without FP/SIMD, so the registers always hold the state
of the last cell that ran. The host remembers which cell owns them and saves
and loads them only when a different cell is about to run, and before a
suspend, fork or freeze of the owner. A cell that yields to the kernel and
comes back pays nothing.

| Option | For | Against |
|---|---|---|
| **Lazy, owner tracked by the host (chosen)** | No cost on system calls or when one cell runs alone | One owner field to keep right |
| Save/restore on every entry and exit | Simplest | ~1 KB copied twice per system call |
| Trap on first use (`CPACR`/`CR0.TS`) | Cells that never use FP never pay | Extra trap path; glibc uses SIMD in `memcpy`, so nearly every cell uses FP |

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
