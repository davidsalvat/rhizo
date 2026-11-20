# Design

## D1. CPUs start only in builds that run cells

The plain boot build prints its banner, memory and ready line and stops
the VM: it has nothing to run on other CPUs, and its boot time is a
benchmarked metric. CPU discovery and bring-up therefore live with the
other cell-only code (`irq`, `user`, `smp` under `ktest`/`bench-cell`).
`cell::init`, called once from `kernel_main` right after the ready line,
sets up the boot CPU's interrupts and address-space tags (formerly done by
the first `Host::new`) and then starts the CPUs, so a secondary CPU copies
a fully configured boot CPU (AArch64 `TCR_EL1` with 16-bit ASIDs).

Lost: starting CPUs in every build (costs boot time for nothing) and
starting them lazily with the first `Host` (the `cpus:` line would land in
the middle of the tests, and benchmarks would time the bring-up).

## D2. Which CPUs: firmware order, boot CPU first, at most 16

`rhizo_core::smp::Cpus::select(boot, firmware, limit)` is a pure function
(property-tested): the boot CPU is number 0 whether or not the firmware
lists it, the firmware's other CPUs follow in order without duplicates, up
to `smp=` and `MAX_CPUS` (16). AArch64 takes `/cpus` (only CPUs PSCI can
start; a single-CPU tree has no enable method). x86_64 takes the MADT:
the RSDP from the PVH start info (`rsdp_paddr`, filled by QEMU's PVH
loader and Cloud Hypervisor), else from the EBDA and `0xe0000..0x100000`
(Firecracker writes it at `0xe0000`). Tables are checksummed and bounded
(64 KiB); pages outside the direct map are mapped read-only on demand.

Without ACPI (QEMU `microvm` with `acpi=off`, which the network tests use
so that `microvm` lists its virtio-mmio devices on the command line), the
CPUs come from the MP configuration table instead (enabled processor
entries; qboot leaves the entry count at zero, so entries are read up to
the base table's length).

Lost: counting CPUs by broadcasting INIT/STARTUP (no list, so no way to
know when all have arrived).

## D3. Bring-up one CPU at a time

Each CPU gets the next number only once it has started: its stacks are
mapped, its start block written, the CPU started, and the kernel waits
(2 s at most) for it to reach `smp::secondary_main` before starting the
next one, so one start block or trampoline data area serves all CPUs. A
CPU that PSCI refuses ends the bring-up with the CPUs online so far; one
that was started but never arrives panics (it might still arrive later on
the same number and stacks as the next one).

- **AArch64**: PSCI `CPU_ON` (HVC or SMC as `/psci/method` says) enters
  `rhizo_secondary_entry` at its physical address with the start block's
  physical address (cleaned to memory: the CPU reads it with caches off).
  It leaves EL2 like the boot CPU, turns the MMU on with the boot identity
  map in `TTBR0` and the kernel's final tables in `TTBR1`, jumps to the
  higher half, takes its stacks and `TPIDR_EL1`, and switches `TTBR0` to
  the empty table.
- **x86_64**: INIT, then STARTUP with vector 8 into a trampoline copied to
  physical `0x8000` (RAM below 1 MiB, which the kernel never hands out,
  checked against the memory map and the boot information); a second
  STARTUP after 10 ms if the CPU has not arrived (the MP specification's
  retry). The trampoline goes from real mode (offsets from CS) to
  protected mode (offsets from its physical base) to long mode on a
  temporary root table: the kernel half shared with the kernel's tables,
  plus the trampoline page mapped at its own address so the CPU keeps
  running when paging comes on. It sets `EFER` (SCE, LME, NXE), the GS
  base, the stack, and calls `rhizo_ap_main`, which loads the kernel's own
  root. The temporary root (four frames) is kept: CPUs start once.

Lost: a parking loop shared by all secondaries in the image (needs
position-dependent code before the MMU is on) and starting all CPUs at
once (needs a start block per CPU for little gain at 16 CPUs).

## D4. Stacks with guard pages in the kernel half

The boot CPU keeps its stacks in the image. CPU `n` gets a 128 KiB window
at `0xffff_ffff_c000_0000 + n * 128 KiB` (the free gigabyte after the
kernel image, in the same top-level entry, so x86_64 cell roots that share
the kernel half see it): a guard page, the 64 KiB kernel stack, a guard,
the 16 KiB exception stack (`SP_EL1` / TSS `RSP0`), a guard and the 16 KiB
double-fault stack (x86_64 IST 1), mapped page by page from fresh frames.
The direct map uses 1 GiB pages and could not hold a guard page.

## D5. Per-CPU data: TPIDR_EL1, and the kernel GS base with SWAPGS

A `PerCpu` record per CPU holds its number and what the cell entry code
used to keep in globals (AArch64: kernel stack pointer, exception stack
top; x86_64: kernel RSP, cell registers, user RSP at `SYSCALL`, trap error
code). AArch64 finds it in `TPIDR_EL1`, which EL0 cannot read.

On x86_64 it is the kernel GS base, set by the entry code before any Rust
code runs. A cell can load GS itself (`mov gs, ax` resets the base), so
the base the kernel relies on must not be live while a cell runs: every
return to a cell (`SYSRETQ`, `IRETQ`) is preceded by `SWAPGS` and every
entry from a cell (`SYSCALL`, an exception from ring 3) starts with it,
leaving the kernel's base in `IA32_KERNEL_GS_BASE` meanwhile. Exceptions
and interrupts taken in ring 0 do not swap. `SWAPGS` is privileged and
`WRGSBASE` is off (`CR4.FSGSBASE` clear), so a cell cannot touch the
kernel's copy. Each x86_64 CPU also has its own GDT and TSS (stacks, and
the TSS descriptor's busy bit); the IDT is shared.

Lost: a fixed GS base without `SWAPGS` (a cell resetting GS would make the
next `SYSCALL` write the kernel's pointers to user memory, and read the
cell-register pointer back from it) and finding the CPU from the APIC ID
(an MMIO or MSR read and a table lookup on every system call).

## D6. Idle with an IPI, no tick

A secondary CPU waits in `wfi` (AArch64) or `sti; hlt; cli` (x86_64) with
its timer off; the boot CPU keeps the periodic tick it had. Work arrives
through a mailbox (function, argument, result) and a wake-up IPI: SGI 0
through `ICC_SGI1R_EL1` (after `dsb ishst`) or fixed vector 33 through the
ICR (after `mfence`, since x2APIC ICR writes do not serialize). An IPI sent
between the mailbox check and the wait stays pending, and both `wfi` and
`sti; hlt` return at once on a pending interrupt, so no wake-up is lost.
In ring 0 vector 33 returns from its stub like the timer; from a cell it
is an interrupt like the timer's. Each CPU finds its GICv3 redistributor by
matching `GICR_TYPER` affinity with its `MPIDR_EL1` (frames 128 or 256 KiB
apart, `VLPIS`), mapped once at bring-up; the boot path still maps only the
boot CPU's.

## D7. One spinlock type, holder known

`sync::Global` keeps its interface and becomes `rhizo_core::sync::SpinLock`:
compare-and-swap of the holder (CPU number + 1) with acquire ordering,
waiting with plain loads, release on exit. The kernel runs with interrupts
masked, so a holder is never interrupted by code that wants the same lock;
the only way for a CPU to find itself as holder is reentrance, which panics
as it did on one CPU. The lock is a library type, so its behavior (no lost
updates with threads as CPUs, reentrance) is tested on the host. Lock order
is fixed by the code that nests them: `SHARED`/`NO_FILES`, then the heap,
then the frame allocator.

Lost: a ticket lock (fair, but no holder to detect reentrance without a
second field; contention is not expected until cells run everywhere, where
`add-smp-scheduling` measures it) and per-CPU allocators (an optimization
that needs a benchmark first, rule 7).

## D8. Heap pages from the top of memory

With several CPUs the order in which frames are taken and freed changes,
and the boot path takes more of them (stacks, redistributors). The kernel
heap keeps the single pages it carves into small blocks; taken next-fit
like every other frame, they ended up scattered over memory, and the free
memory between them had no 2 MiB-aligned block for a copy-on-write large
page, nor a run of a few MiB for a snapshot: a template child was killed on
a write to a 2 MiB page (CI, AArch64, four vCPUs). Single heap pages now
come from the top of memory (`FrameAllocator::alloc_high`), so they pile up
there and leave the rest in large runs; cells, tables and large heap
allocations keep taking frames next-fit from the bottom. The host's run
queue holds boxed cells: a thousand live cells need a few KiB of queue, not
a megabyte of contiguous heap taken from the kernel's reserve when memory is
nearly all given to cells (`a_thousand_idle_cells` failed on that once heap
pages moved to the top).

Fragmentation must not kill a cell either: a write to a shared 2 MiB page
that finds no free 2 MiB block copies the page into 512 small pages under
a new table. This change found it on the AArch64 runner and fixed it;
`add-sockets-and-poll` landed the same fallback on `main` meanwhile, so
the merge keeps `main`'s code and this change adds its requirement to the
`fork` spec.

Lost: returning empty heap pages to the allocator (the heap keeps them by
design, `add-memory-accounting`), and mapping large heap allocations from
scattered frames into a virtual window (unmapping them would need TLB
shootdowns of kernel addresses on every CPU).
