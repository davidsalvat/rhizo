# Proposal

## Why

On x86_64 a cell thread's FP/SIMD state is loaded with `FXRSTOR`
(`fp_load`, on the thread's first FP/SIMD instruction after a switch). On
AMD and Hygon CPUs without `XSAVEERPTR` (`CPUID.8000_0008:EBX[2]`, absent
before Zen 2), `FXRSTOR` does not load the x87 last-instruction opcode
(`FOP`), instruction pointer (`FIP`) and data pointer (`FDP`) unless the
image has an x87 exception pending. A cell that reads them (`FNSTENV`,
`FNSAVE`, `FXSAVE`) then finds the last x87 instruction and operand
addresses of the thread or cell that ran before it on that CPU: the
CVE-2006-1056 class of leak, enough to defeat another cell's address
randomization or follow what it computes. Linux handles it with
`X86_BUG_FXSAVE_LEAK`.

Bringing up the in-kernel test showed the same leak under QEMU's emulation
(TCG, the `-cpu max` model `xtask` runs, QEMU 8.2): it reports
`AuthenticAMD` *with* `XSAVEERPTR`, yet `FXRSTOR` never loads `FIP` and
`FDP`, so `FNSTENV` in the next thread reads the previous one's. A rule on
the CPUID feature bit alone would leave it open there.

## What Changes

- At boot, x86_64 decides once whether `FXRSTOR` leaks the x87 pointers:
  AMD or Hygon vendor without `XSAVEERPTR`, or QEMU's emulation (hypervisor
  signature `TCGTCGTCGTCG`). The answer is a flag.
- Where it leaks, `fp_load` first clears pending x87 exceptions, empties
  the x87 stack and loads an integer from a kernel variable
  (`fnclex; emms; fild`), so the pointers name kernel code and data, the
  same for every cell, before `FXRSTOR` loads the thread's state.
  Elsewhere the load costs one flag read.
- New isolation requirement: FP/SIMD registers hold nothing of another
  thread.
- In-kernel tests: the flag follows CPUID, and an x87 load's pointers do
  not survive loading another thread's state (fails under TCG without the
  scrub).
- AArch64 is not affected and does not change.

## Capabilities

- `isolation`: FP/SIMD registers hold nothing of another thread.

## Impact

- `kernel/src/arch/x86_64/user.rs` (`fp_load`, `init_tags`, vendor check
  shared with `kpti_needed`), `kernel/src/arch/x86_64/tests.rs`,
  `docs/security.md`.
- Under QEMU TCG (CI's `-icount` gates) the scrub runs: four more
  instructions per FP/SIMD load, which happens only when a thread that
  uses FP/SIMD runs after another did on that CPU. The gates pass (see
  design D3).
