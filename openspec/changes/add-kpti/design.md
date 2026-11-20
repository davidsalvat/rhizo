# Design

## D1. When KPTI is on

`user::kpti_needed()` decides as Linux does: no on AMD and Hygon CPUs
(not affected by rogue data cache load), no on Intel CPUs whose
`IA32_ARCH_CAPABILITIES` has `RDCL_NO`, yes otherwise. `cell::init` turns
it on accordingly; the test build turns it on always, so every in-kernel
test (on QEMU, Firecracker and Cloud Hypervisor, one CPU and four) runs
with it. `user::set_kpti` switches it while no cell is live (the
benchmark measures both modes in one boot; one test runs cells both ways).

Alternative: always on. Rejected: on CPUs that are not affected it costs
two CR3 writes per system call for nothing, and Rhizo's thesis is speed;
Linux makes the same choice.

## D2. One kernel half for every cell: the entry pages

With KPTI the kernel runs on its own tables (`memory::kernel_tables()`,
PCID 0) for everything, system calls included: it never touches a cell's
lower half directly (copies go through the direct map), so it does not
need the cell's tables loaded. A cell's root carries, in its upper half,
the entries of one set of *entry tables* built at start
(`user::init_kpti`), mapping at their kernel addresses, supervisor only:

- the entry code: two page-aligned sections (`.text.rhizo_entry_a`, the
  exception stubs; `.text.rhizo_entry_b`, entry, return, system call),
  executable, read-only;
- every CPU's per-CPU data, the GDTs and TSSs, the IDT (page-aligned
  statics, alone on their pages);
- every CPU's trap and double-fault stacks (where the CPU pushes on an
  exception from ring 3).

Nothing else: not the direct map, heap, kernel data, code or tables. A
cell's root is still one frame; `fork` copies the parent's upper half as
before, so forks and templates need nothing new.

The entry code:

- `SYSCALL`: `SWAPGS`, save the cell's RSP in per-CPU data, load the
  kernel's CR3 from per-CPU data (`gs:[40]`, using RSP as scratch), then
  continue as before.
- Exception or interrupt from ring 3: `SWAPGS`, push RAX on the trap
  stack, load the kernel's CR3 through RAX, continue as before.
- Return: the cell's RDI is stashed in per-CPU data (`gs:[56]`), every
  other register loaded from `Registers` while the kernel's tables are
  still loaded, then CR3 = the cell's (`gs:[48]`, through RDI), RDI from
  per-CPU data, `SWAPGS`, `SYSRETQ`. For `IRETQ` the frame is built on the
  trap stack (`gs:[64]`), which the cell's tables map, instead of the
  kernel stack.

A zero CR3 in per-CPU data means no switch: with KPTI off the paths run
as before, plus two `test`/`jz`.

PCIDs: the kernel's CR3 is loaded with bit 63 (keep PCID 0's entries),
the cell's with bit 63 unless it must flush, so neither switch flushes the
TLB. An untagged cell shares PCID 0 with the kernel: its CR3 is always
loaded without bit 63, so no kernel translation stays cached while it
runs. Kernel pages are never global.

Alternative: two roots per cell (a kernel view with the cell's lower half
and a user view), as Linux's 8 KiB PGD. Rejected: both roots' lower-half
entries must then be kept equal by every page-table change in `rhizo-mm`,
and every cell costs a frame more (`cell.pt_frames`), for nothing the
kernel needs.

## D3. TLB flushes with KPTI

`INVLPG` in the kernel acts on PCID 0, not on the cell's. When the kernel
flushes a cell's translation (`munmap`, `mprotect`, copy-on-write,
`fork`), the cell is marked stale on every CPU (`Live::clean = 0`), and
each CPU flushes the cell's PCID on its next entry (CR3 without bit 63).
`flush_user` no longer loads the cell's tables in the kernel (they map no
kernel code).

Alternative: `INVPCID` (single address) where the CPU has it. Rejected
for now: one more code path for a cost the benchmark does not show; it is
the way to cut flushes if `cell.fork_touch_us` with KPTI matters.

## D4. AArch64: E0PD

The kernel lives in `TTBR1_EL1` and was never in a cell's tables, but a
Meltdown-affected core can still speculate through TTBR1 translations
from EL0. FEAT_E0PD (`TCR_EL1.E0PD1`) makes every EL0 access to the TTBR1
half fault in constant time, whatever the TLB holds: Linux uses it
instead of KPTI. `user::init_cpu` sets it on every CPU that has it; it
costs nothing, so it is always on. A CPU with neither E0PD nor `CSV3` (not
affected) is reported at start; Rhizo has no trampoline for it (Cortex-A75
is the only such Arm core).

## D5. Cost, measured in every run

The `bench-cell` kernel measures, in its default mode, `cell.syscall_ns`
(a cell times 20000 `getpid` calls with `clock_gettime`), and then with
KPTI forced on `cell.syscall_kpti_ns`, `cell.switch_kpti_ns` and
`cell.run_kpti_us` (the `cell.switch_ns` and `cell.run_us` workloads). So
the cost is visible even on CI runners whose CPUs do not need KPTI.
