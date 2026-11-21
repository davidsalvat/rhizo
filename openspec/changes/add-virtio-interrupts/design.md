# Design

## D1. Which interrupt

Each virtio-mmio transport names its interrupt: the first `interrupts`
specifier of its device-tree node on AArch64 (an SPI, GIC interrupt
identifier 32 + n), and the third field of `virtio_mmio.device=` on x86_64
(an I/O APIC input). `platform::for_each_virtio_mmio` passes it with the
base. The card enables its interrupt (`VirtioNet::enable_interrupts`, used
buffers notify) only once the platform routed it. A card on PCI has no
routed interrupt and is polled as before (D4 of `add-virtio-net`): MSI-X
needs its own capability parsing and vectors, and is left for a later change.

## D2. Routing it to the boot CPU

Only the boot CPU polls the card, so the interrupt goes there.

- AArch64: the distributor sets the SPI to group 1, middle priority,
  edge-triggered, routed to the boot CPU's affinity (`GICD_IROUTER`), and
  enabled. The CPU interface already takes group 1 interrupts for the
  preemption timer.
- x86_64: the I/O APIC (at `0xfec0_0000` on QEMU `microvm` and Firecracker,
  mapped on first use) routes the input to the boot CPU's local APIC on
  vector 35, fixed delivery, edge-triggered, active high. An input the I/O
  APIC does not have is refused.

Both paths treat the interrupt as the timer's: in a cell it ends the slice
(`Trap::Interrupt`), in the idle wait it only wakes the CPU. The kernel
acknowledges the card's interrupt status before each poll, so a frame that
arrives after it raises the interrupt again.

Edge-triggered: a level-triggered line would stay up until the poll, and
interrupt every cell slice meanwhile.

## D3. When the boot CPU polls

With an interrupting card, the scheduler's poll (`poll_wire`) runs when:

- `smoltcp`'s delay ended (its retransmission and keep-alive timers, DHCP);
- the card holds received frames not taken yet (`VirtioNet::pending`, a read
  of the used ring in memory, no register): the interrupt woke the CPU;
- a system call gave `smoltcp` something to send (`Net::take_wire_work`):
  a `connect`, `send` or `sendto` on the card, a read that opened room in
  a receive window, a close. A call on another CPU wakes the boot CPU for it.

The last two set `WIRE_DUE`, the time of the next poll, to 0: a system
call as it ends (under the sockets' lock, when `Net::end_call` reports work
for the wire), and any CPU after an interrupt or a wait for one, when
frames wait in the card's ring. The scheduler's check at every step stays a
single load and comparison, inlined; the poll is out of line, and a poll
that started before a 0 was stored does not overwrite it.

The 20 us poll while connections wait (`BUSY_NS`) and the 1 ms poll for frames
nobody waits for apply only to cards without interrupts.

Lost: polling on every scheduling step while connections wait (the
interrupt says when to); a separate interrupt handler that polls the card
(the kernel runs with interrupts masked, and the next scheduling step comes
right after the interrupt anyway).

## D4. Measuring it

`laptop.turn_wakeups` counts the boot CPU's returns from waiting for an
interrupt during the agent turns of the laptop workload, divided by the
turns: with interrupts, about one per turn (the answer). The in-kernel test
`the_cards_interrupt_wakes_the_idle_cpu` checks the same on a virtio-mmio card:
20 round trips with the host's echo server wake the CPU at most 4 times each.
