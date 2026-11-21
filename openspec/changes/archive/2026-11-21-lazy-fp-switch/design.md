# Design

## Context

Before this change (design D1 of `add-real-workloads`), each CPU had one
owner of its FP/SIMD registers. A thread that was not the owner ran with
FP/SIMD trapping (`CR0.TS`/`#NM` on x86_64, `CPACR_EL1.FPEN = 0b01` on
AArch64) on registers that still held the owner's values; its first
FP/SIMD instruction trapped, the kernel saved the owner (found under the
scheduler's lock when it was a waiting cell) and loaded the thread. With
several CPUs a cell's state left the CPU with the cell, so every slice of
a thread that uses FP/SIMD began with that trap.

Measured before the change, in retired instructions under `-icount`
(`cargo xtask bench --icount-only`, x86_64 `qemu-microvm`, AArch64
`qemu-virt`):

| Figure | x86_64 | AArch64 |
|---|---|---|
| `cell.switch_insns` (two `yielder` cells, no FP/SIMD) | 896 | 838 |
| `cell.switch_fp_insns` (two `fp_regs` cells, FP/SIMD in every turn) | 2022 | 2248 |
| `cell.syscall_insns` | 336 | 284 |

A switch between cells that never use FP/SIMD did no save or load at all:
its FP/SIMD part was the owner check and the trap-state check, about a
dozen instructions (1.5 %). A system call did none either. The cost was in
switches between cells that use FP/SIMD, which is almost every real
program (glibc's string functions use SSE2 and Advanced SIMD): each turn
paid one more exception round trip, the owner lookup, a save and a load.
The new figure `cell.switch_fp_insns` measures that path (it includes
`fp_regs`'s own work per round, the same before and after).

## Decisions

### D1. Eager for threads that use FP/SIMD, clear registers for the others

Each thread's `Registers` carry `fp_used`, set by its first FP/SIMD trap.
As a thread is switched in on a CPU (`fp_enter`):

- it owns the registers already: nothing to do (FP/SIMD is enabled);
- it has used FP/SIMD: its state is loaded and FP/SIMD enabled;
- it has not, and the registers are clear (`FP_CLEAN`): nothing to do
  (FP/SIMD traps);
- it has not, and they hold anything else: they are loaded with the
  initial state (`FpState::initial`, what a program starts with) and
  FP/SIMD traps. Its first FP/SIMD instruction loads its own state and sets
  `fp_used`.

A thread's state is saved when it stops running on a CPU: at the end of
its turn, when it blocks and another thread of the cell runs in the same
slice, when a gang thread is put back. With one CPU the registers stay the
thread's (owner unchanged), so the same thread running next needs no
load; with several, the owner becomes `FP_STALE` (another CPU may change
the thread's state), and a stale owner is always loaded over or cleared.
Saving at the stop, not when the next thread needs the registers, removes
the owner lookup and keeps every waiting thread's state in memory for
suspend, fork and freeze.

Trapping follows the owner: enabled while a thread owns the registers,
trapping while they are `FP_CLEAN` (either while `FP_STALE`). The common
cases then write no control register: two non-FP threads taking turns
find `FP_CLEAN` and trapping already on; an FP thread that runs again
finds itself the owner. x86_64's `fp_save` and `fp_load`, which must
clear `CR0.TS` to run `FXSAVE`/`FXRSTOR`, put it back as it was, as
AArch64's never change `CPACR_EL1`: a save or load outside switching (the
in-kernel tests that read the registers) cannot leave FP/SIMD enabled
on `FP_CLEAN` registers. The switching paths enable FP/SIMD before they
load, so no `CR0` write is undone. Every load, the clearing included,
goes through `fp_load`, which is where `scrub-x87-pointers` overwrites
the x87 pointers on CPUs whose `FXRSTOR` keeps them.

| Option | For | Against |
|---|---|---|
| **Eager for FP users, clear registers otherwise (chosen)** | No trap for FP programs; nothing for non-FP ones; nothing of another thread in the registers | A save at every stop of an FP thread, even when it runs again next with one CPU (one instruction on x86_64, 21 on AArch64) |
| Keep trap on first use, owner's values left in place | Cheapest for one FP cell among non-FP cells | LazyFP; a trap per turn for every FP program |
| Eager for every thread (Linux) | Simplest | A load and a save per switch for threads that never use FP/SIMD: `cell.switch_insns` up on AArch64 (V0-V31 are 32 loads), and the first version of D1 of `add-real-workloads` measured 303 → 572 ns on KVM for such cells |
| `XSAVEOPT`/`XSAVES` with the init and modified optimizations | Saves skip unused or unmodified components | Needs `CR4.OSXSAVE` and the XSAVE layout (snapshot format, `Registers` layout) for a 512-byte legacy state; still a save and a load per switch; x86_64 only |
| `CR0.TS` cleared lazily on return (Linux's `TIF_NEED_FPU_LOAD`) | Defers the load past kernel work | The kernel never uses FP/SIMD, so there is nothing to defer: `fp_enter` already runs right before entering the cell |

### D2. LazyFP

LazyFP (CVE-2018-3665) needs another context's values in the registers
while FP/SIMD traps: an FP/SIMD instruction that runs speculatively before
the trap reads them. With D1 the registers hold only the running
thread's state or the initial state whenever cell code runs, on every CPU
and in gangs, so there is nothing to read, whatever the CPU. This holds on
AArch64 too: the same kernel code decides for both architectures, so the
design does not depend on whether a given Arm core speculates past an
`FPEN` trap. The kernel itself is built without FP/SIMD, so it never puts
its own values there. KPTI changes nothing here: the registers are not
memory.

The in-kernel test `fp_registers_hold_nothing_of_another_cell` checks the
property directly: after a scheduling step in which a cell that never used
FP/SIMD ran right after `fp_regs` filled every register, the kernel reads
the CPU's registers (`fp_save`) and finds the initial state. Before this
change it found `fp_regs`'s values.

### D3. The hint is not cell state

`fp_used` sits in what was padding in `Registers` (offset 280; the entry
code's offsets do not change). It is a whole word (0, or 1 once used), not
a `bool`: a `bool` gives `Registers` a niche, which changes the layout of
every `Option` and `Result` that holds a thread or a cell; measured, it
cost AArch64's `cell.spawn_insns` 2.4 % (51536 → 52782) and
`cell.run_insns` 1 %, while a word leaves both as they were. It is a scheduling hint, not part of what a
thread computes: snapshots leave it out, and a restored thread starts
without it, traps once on its first FP/SIMD instruction and loads its
state from the snapshot. A thread that has not touched FP/SIMD since its
restore therefore still has the restored state in memory, and a second
suspend captures it unchanged. Forks, templates and new threads copy the
registers and so the hint. The snapshot format does not change. Once set,
the hint stays: a thread that used FP/SIMD once is switched eagerly for
good, as Linux switches every thread.

### D4. Tests and the benchmark

- `fp_regs` (C, glibc, threads): each thread loads distinct values into
  every vector register and the FP control and status registers (x86_64:
  XMM0-XMM15, `MXCSR`, x87 control word; AArch64: V0-V31, `FPCR`, `FPSR`),
  makes a system call (yield, or a sleep that blocks it) in the same
  `asm` block, and checks every register after it; it also runs on Linux
  (`cells/expected.rs`, differential check).
- `fp_peek` (Rust, no FP/SIMD of its own): a hostile reader whose first
  FP/SIMD instruction reads the registers and which only ever reads them;
  it must find the initial state every time.
- In-kernel tests (run on 1 CPU in `ktest` and on 4 in `ktest-smp`): the
  register check of D2; four `fp_regs` cells (two with three threads that
  also block) next to two `fp_peek` cells and a `yielder`; a two-thread
  `fp_regs` cell suspended and resumed every few steps (eagerly and
  lazily, every third time twice in a row) and forked mid-run, next to
  another `fp_regs` cell and an `fp_peek` cell.
- `cell.switch_fp_ns`: two `fp_regs` cells yielding to each other 2000
  times each, minus the same run with no yields, per switch; counted in
  instructions under `-icount` like every timed figure.

## Risks

- A path that stops running a thread without saving it would lose its
  state, and a path that changes the owner without the trap state would
  break D1's invariant. The stops are few (end of turn, block, gang put
  back, cell end, share), each saves explicitly, and the tests run threads
  that block, gangs on 4 CPUs, suspends, resumes and forks.
- An FP thread now pays a save at the end of every turn with one CPU, even
  when no other thread needs the registers. It is one instruction on
  x86_64 and 21 on AArch64, against the trap it no longer takes.
