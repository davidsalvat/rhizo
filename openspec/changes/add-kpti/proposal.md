# Proposal

## Why

On x86_64 the kernel half is mapped, supervisor only, in every cell's
tables (open item O3 of `docs/security.md`). On a CPU open to Meltdown
(rogue data cache load, CVE-2017-5754) a cell can read any kernel memory
through speculation: other cells' pages through the direct map, the
kernel's heap, snapshots. Supervisor-only is no barrier there. The usual
answer is KPTI: while a cell runs, the kernel is not mapped.

## What Changes

- x86_64 KPTI: a cell's tables carry, in their kernel half, only the entry
  pages (entry code, per-CPU data, GDTs and TSSs, IDT, trap and
  double-fault stacks of every CPU), mapped supervisor only from one set of
  tables shared by every cell. The entry code switches CR3 to the kernel's
  tables first thing on a system call, exception or interrupt from a cell,
  and back to the cell's last thing before `SYSRETQ`/`IRETQ`. The kernel
  keeps PCID 0, each cell its own PCID: no TLB flush on either switch.
- With KPTI, a TLB flush the kernel makes for a cell (`munmap`,
  `mprotect`, copy-on-write, fork) cannot reach the cell's PCID: the cell
  is marked stale on every CPU and its next entry flushes its PCID.
- The default follows the CPU, as Linux: on where Meltdown applies (Intel
  CPUs without `RDCL_NO`, and unknown vendors), off on AMD and Hygon CPUs
  and Intel CPUs that report `RDCL_NO`. Every in-kernel test runs with it
  on, and one runs cells both ways.
- AArch64: the kernel already lives in `TTBR1_EL1`, out of a cell's tables;
  where the CPU has FEAT_E0PD, `TCR_EL1.E0PD1` makes EL0 accesses to the
  kernel half fault in constant time, which is what KPTI buys. A CPU
  without E0PD that does not report `CSV3` (Cortex-A75 is the known one)
  is reported at start, not protected.
- Benchmarks: `cell.syscall_ns` (round trip of `getpid`), and with KPTI
  forced on `cell.syscall_kpti_ns`, `cell.switch_kpti_ns` and
  `cell.run_kpti_us`, in every run, whatever the CPU's default.

### Out of scope

Other speculation attacks (Spectre v1/v2, L1TF, MDS and their
mitigations: IBRS/IBPB, retpolines, `VERW` on return), a KPTI trampoline on
AArch64 for CPUs without E0PD, hiding the entry pages themselves (they
hold no secret: the kernel's layout is public, there is no KASLR).

## Capabilities

### Modified Capabilities

- `isolation`: cells cannot reach the kernel by speculation.
- `benchmarks`: KPTI metrics.

## Impact

`kernel/src/arch/x86_64/{cpu,user,tests}.rs` (entry code, per-CPU data,
entry tables), `kernel/src/arch/aarch64/user.rs` (E0PD),
`kernel/src/cell.rs` (stale-everywhere flushes, KPTI benchmarks),
`kernel/src/tests.rs`, `xtask/src/bench.rs`,
`bench/README.md`, `docs/security.md`. The boot image is unchanged in
size beyond a few instructions (page alignment only in builds that run
cells). With KPTI off, entering a cell and system calls run the code
from before (a second copy of each); exceptions gain one test of a zero
CR3.
