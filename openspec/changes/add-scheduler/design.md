# Design

## Context

H2 runs one cell until it exits: `enter_user` returns on a system call or a
fault. Interrupts are masked everywhere and no interrupt controller is set
up. Cells are plain data (`Cell`), so a run queue of cells is plain data too.

## Goals / Non-Goals

Goals: many live cells, preemption of untrusted code, sleeping without CPU,
isolation proven by tests, cheap switches. Non-goals: SMP, priorities,
signals, real-time guarantees.

## Decisions

### D1. Scheduling logic in a host-tested crate

`rhizo-sched` holds `Scheduler<T>`: a FIFO run queue and a sleep list of
`(wake_at_ns, T)`. Operations: `add`, `next(now)` (moves woken sleepers to
the queue, pops the next runnable), `requeue`, `sleep(t, until)`,
`next_wake()`. Time is a parameter, never read, so tests are deterministic
(spec rule 2). The kernel stores its `Cell`s in it.

### D2. Interrupts only from cell code

The kernel runs with interrupts masked, as today; cells run with them
unmasked (`SPSR.I = 0`, `RFLAGS.IF = 1`). A timer interrupt therefore always
arrives from user mode and leaves through the same path as a fault
(`enter_user` returns `Trap::Interrupt`). No kernel code is reentrant and no
locking appears.

| Option | For | Against |
|---|---|---|
| **Interrupts only in user mode (chosen)** | No reentrancy, no locks, tiny entry code | Kernel work is never preempted (fine: it is short and bounded) |
| Preemptible kernel | Lower latency for long kernel work | Locks, interrupt-safe allocator, far more code |

### D3. Timers and interrupt controllers

- AArch64: virtual timer (`CNTV_*`, PPI 27) through a GICv3, found in the
  device tree (`arm,gic-v3`: distributor and redistributor). QEMU `virt` is
  started with `gic-version=3`, matching Firecracker and Cloud Hypervisor.
  The CPU interface uses system registers (`ICC_*`).
- x86_64: local APIC timer in one-shot mode, through x2APIC MSRs when the CPU
  has them, else the xAPIC page (mapped as device memory). The legacy PIC is
  masked. Vector 32.

### D4. Time slice: a periodic tick

The timer is a periodic tick every 10 ms, armed once; a cell runs until the
next tick. It is not re-armed when cells switch.

| Option | For | Against |
|---|---|---|
| **Periodic tick (chosen)** | Switching cells touches no timer register | A cell switched in just before a tick gets a short first slice |
| Re-arm a full slice at every switch | Exact slices | One timer write per switch: under virtualization a VM exit each (measured in CI: switches took ~8 µs, `cell.run_us` doubled) |

Waiting for a sleeper's deadline while idle programs a one-shot for that
deadline and restores the tick afterwards. On AArch64 the counter frequency
is known (`CNTFRQ`). On x86_64 the APIC timer of KVM and QEMU counts at
1 GHz; on other hardware the period would differ, but cells are still
preempted. The spec only requires that a running cell is preempted, not an
exact slice.

### D5. Sleeping and idling

`nanosleep` moves the cell to the sleep list. When no cell is runnable, the
kernel arms the timer for the earliest wake-up and waits (`wfi` / `hlt`)
with interrupts enabled at that point only; the interrupt is acknowledged and
scheduling resumes.

### D6. Address-space identifiers

| Option | For | Against |
|---|---|---|
| **Tag per live cell, flush on reuse (chosen)** | No flush on switch; one targeted flush per cell lifetime | Tag allocator (a bitmap) |
| Generation-based rollover (Linux) | Unbounded cells | More code; not needed below 4096/65536 live cells |
| Flush on every switch (H2) | Simplest | Every switch refills the TLB |

AArch64: `TCR_EL1.AS = 1` when `ID_AA64MMFR0_EL1.ASIDBits` says 16 bits,
otherwise 8-bit ASIDs; ASID 0 stays for the empty table. x86_64: `CR4.PCIDE`
when CPUID reports PCID; PCID 0 for the kernel's own tables; switches set
bit 63 of CR3 (no flush). Without PCID, every switch flushes, as in H2.
Ending a cell flushes its tag (`TLBI ASIDE1IS`; on x86_64 a CR3 load of the
dead cell's PCID without bit 63, then back) before the tag is reused.

### D7. Kernel API

`cell::Host` keeps the scheduler and the cells. `spawn` adds a cell;
`run(until)` runs cells until none is left or until a predicate on the
finished cells holds; `kill(id)` ends a cell. Tests and the benchmark use
it; the H2 single-cell `run` becomes a wrapper.

## Risks / Trade-offs

- [An interrupt in the window between unmasking and EL0/ring 3] → unmasking
  happens through `ERET`/`IRETQ` itself (SPSR/RFLAGS), never in kernel code.
- [x86_64 slice length uncalibrated without TSC-deadline] → bounded, not
  exact; documented.
- [GICv3 only] → all three AArch64 VMMs provide it with the options used.
