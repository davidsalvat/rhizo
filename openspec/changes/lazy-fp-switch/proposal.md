# Proposal

## Why

The kernel switches FP/SIMD state on first use (design D1 of
`add-real-workloads`): a thread that does not own the CPU's FP/SIMD
registers runs with FP/SIMD trapping (`CR0.TS` on x86_64,
`CPACR_EL1.FPEN = 0b01` on AArch64), and only its first FP/SIMD
instruction saves the owner's state and loads its own. That has two costs:

- **Security.** While the trap is armed, the registers still hold the
  previous owner's values, often another cell's. On CPUs open to LazyFP
  (CVE-2018-3665, Intel Core parts), an FP/SIMD instruction that runs
  speculatively before the trap is taken reads them, and a cell can leak
  another cell's registers through a cache side channel. Linux and the
  BSDs restore FP state eagerly for this reason.
- **Speed for real programs.** Almost every real program uses FP/SIMD:
  glibc's `memcpy`, `memset` and `strlen` use SSE2 and Advanced SIMD.
  Every time two such cells take turns on a CPU, each pays a trap: one more
  exception round trip into the kernel, a lookup of the owner (under the
  scheduler's lock when it is a waiting cell), a save and a load. With
  several CPUs every slice starts with that trap, because a cell's state
  leaves the CPU with the cell.

Cells that never use FP/SIMD (the Rust test cells behind `cell.switch_ns`)
pay almost nothing today, and must keep paying nothing.

## What Changes

- **Eager for threads that use FP/SIMD, nothing for those that do not.**
  Each thread's registers carry a hint, `fp_used`, set at its first
  FP/SIMD trap. A thread with the hint gets its state loaded as it is
  switched in, with FP/SIMD enabled: no trap. A thread without it runs
  with FP/SIMD trapping, on registers that hold the initial state; its
  first FP/SIMD instruction loads its state and sets the hint.
- **The registers never hold another thread's values while cell code
  runs.** A thread's state is saved as it stops running on a CPU (end of
  its turn, a block that lets another thread of the cell run, a gang
  thread put back); the next thread either loads its own state or, if it
  has not used FP/SIMD, finds the registers cleared to the initial state
  first. With one CPU the registers stay the last thread's until another
  needs them, so a thread that runs again next needs no load. This closes
  LazyFP on both architectures without relying on the CPU being immune.
- **Trapping follows the owner.** FP/SIMD is enabled exactly while a
  thread owns the registers and trapping while they are clear, so a
  switch between threads that do not use FP/SIMD writes no control
  register, and x86_64's `fp_save` and `fp_load` leave trapping as they
  found it (AArch64's never touch it), so a load or save outside
  switching, as a test does, cannot break that.
- **The hint is not state.** Snapshots leave `fp_used` out: a restored
  thread traps once, as a new one does, then is switched eagerly. Forks,
  templates and new threads copy it with the registers. Snapshot format,
  the FP/SIMD layout and `rhizo-snapshot` do not change.
- **Tests.** `fp_regs` (C, glibc, threads): every FP/SIMD register, and
  the FP control and status registers, held with distinct values across
  yields and sleeps, checked after each; `fp_peek` (Rust): a hostile cell
  whose first FP/SIMD instruction reads the registers, and which only
  ever reads them. In-kernel tests, run on 1 CPU (`ktest`) and 4
  (`ktest-smp`): the registers read by the kernel after a non-FP cell ran
  right after `fp_regs` hold the initial state; cells and threads with
  distinct values next to hostile readers; a threaded cell suspended,
  resumed (sometimes twice before it runs) and forked mid-run.
- **Benchmark.** `cell.switch_fp_ns` (and `_insns` under `-icount`): two
  `fp_regs` cells yielding to each other.

### Out of scope

- XSAVE, AVX and SVE state: cells get SSE and Advanced SIMD only, as now
  (`CR4.OSXSAVE` and `CPACR_EL1.ZEN` stay off).
- AMD's `FXSAVE` pointer leak (x87 `FIP`/`FDP`/`FOP` not restored by
  `FXRSTOR` without a pending exception on CPUs without `XSAVEERPTR`, and
  under QEMU's emulation): change `scrub-x87-pointers` scrubs them inside
  x86_64's `fp_load`. Every path this change loads FP/SIMD state on (a
  thread switched in, a first use, clearing to the initial state) goes
  through `fp_load`, so that guarantee holds on all of them.
- Clearing the hint of a thread that stopped using FP/SIMD: a thread that
  used it once is switched eagerly for good, as Linux switches every
  thread.

## Capabilities

- `isolation`: FP/SIMD registers hold nothing of another thread, not even
  for speculation.
- `benchmarks`: `cell.switch_fp_ns`.

## Impact

- `crates/rhizo-cell`: `Registers::fp_used` (a word in padding at offset
  280; the layout the entry code uses is unchanged), `FpState::initial`
  const.
- `kernel/src/cell.rs`: switching (`fp_enter`, `fp_first_use`, saves when a
  thread stops running), the benchmark; `kernel/src/arch/x86_64/user.rs`:
  `fp_save` and `fp_load` keep trapping as it was.
- `cells`: `fp_regs.c`, `fp_peek.rs`; `cells/expected.rs`.
- `xtask`: `cell.switch_fp_ns`; `bench/README.md`, `bench/baseline.json`
  (new entries only); `docs/security.md`.
