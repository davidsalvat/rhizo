# Proposal

## Why

The kernel finds out that frames reached the network card only by polling it
(design D4 of `add-virtio-net`). While a cell waits for the network, the boot
CPU wakes every 20 us to poll the card, and every millisecond while nothing
waits. Each agent turn of the laptop workload therefore costs dozens of
wake-ups while the agent waits for the model's answer, and an answer that
arrives between two polls waits for the next one.

## What Changes

- **The card interrupts.** A virtio-mmio card whose interrupt the platform
  names (the device tree on AArch64, `virtio_mmio.device=` on x86_64) gets
  that interrupt routed to the boot CPU: the GICv3 distributor on AArch64,
  the I/O APIC on x86_64 (a new device vector, 35). The interrupt wakes the
  boot CPU from its wait, or interrupts the cell it runs. The kernel
  acknowledges it before each poll of the card.
- **No polling for frames.** With an interrupting card, the boot CPU polls
  the card when its interrupt came, when received frames wait in the card's
  ring, when a system call gave `smoltcp` something to send (bytes, a SYN, a
  FIN, room in a receive window), and at `smoltcp`'s own timers. It no
  longer polls every 20 us while connections wait, nor every millisecond.
- **A call that gives `smoltcp` work** on another CPU wakes the boot CPU, which
  polls the card.
- **Cards without interrupts** (virtio-pci: QEMU `q35`, Cloud Hypervisor,
  AArch64 with 4 CPUs) are polled as before. MSI-X is left for a later change.
- **Metric**: `laptop.turn_wakeups`, the boot CPU's returns from waiting for
  an interrupt per agent turn.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `networking`: the card's interrupt.
- `benchmarks`: wake-ups per agent turn.

## Impact

`crates/rhizo-virtio` (`pending`), `crates/rhizo-net` (`take_wire_work`),
`kernel/src/arch/*/irq.rs` (`route_device`), `kernel/src/arch/*/platform.rs`
(the transports' interrupts), `kernel/src/arch/x86_64/{cpu,user}.rs` (the
device vector), `kernel/src/wire.rs`, `kernel/src/cell.rs`,
`kernel/src/workloads.rs`, `xtask/src/workloads.rs`, `bench/README.md`.
