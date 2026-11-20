# Design

## Decisions

### D1. One driver, two transports

The device logic of `VirtioNet` (status handshake, feature negotiation,
split virtqueues, buffers) is the same on virtio-mmio and virtio-pci; only
where the registers are differs. A private `Transport` enum (`Mmio`,
`Pci`) inside `rhizo-virtio` provides the few operations the driver needs
(status, features, queue setup, notification, MAC address), so
`VirtioNet<R, M>` keeps its type and the kernel keeps one `Card` type for
both transports.

| Option | Cost |
|---|---|
| **enum inside the driver (chosen)** | one match per register access, no new type parameters |
| a `Transport` trait, `VirtioNet<T, M>` | a second generic layer and two card types in the kernel |
| a second driver for PCI | duplicated queue logic |

virtio-pci registers have their own widths (8-bit `device_status`, 16-bit
queue registers and notifications) and Cloud Hypervisor rejects other
widths, so `DeviceRegisters` gains `read8/write8/read16/write16`. The
configuration space itself is read by dwords, which ECAM always allows.

### D2. Finding the function: ECAM, bus 0

Every target VMM exposes PCI configuration space through ECAM: QEMU `q35`
and Cloud Hypervisor list it in the ACPI MCFG (x86_64), QEMU `virt` and
Cloud Hypervisor in a `pci-host-ecam-generic` device-tree node (AArch64).
`rhizo-core` reads both (`acpi::pci_ecam`, `Fdt::pci_memory_window`), so
legacy port I/O configuration access (0xCF8) is not needed. The kernel
scans function 0 of the 32 devices of bus 0, which is where all of them
put a `-device virtio-net-pci` or `--net` card; bridges and other buses
are left for when a VMM needs them.

A function is a virtio network device when its vendor is 0x1af4 and its
device ID 0x1041 (modern) or 0x1000 with subsystem ID 1 (transitional;
`xtask` asks QEMU for modern-only cards, but QEMU's default stays
supported). Its vendor capabilities give the BAR, offset and length of the
common, notification (with `notify_off_multiplier`) and device structures;
the first of each type is used, and the walk stops after 48 capabilities
in case of a cycle.

### D3. BARs: assigned when nobody else did

On x86_64, SeaBIOS (`q35` boots the kernel through it) and Cloud
Hypervisor assign every BAR; the kernel keeps their addresses
(`platform::pci_window` is `None`). On AArch64, QEMU `virt` jumps to the
kernel without firmware, so BARs are zero: the kernel gives them naturally
aligned addresses from the host bridge's 32-bit memory window (the
`ranges` entry of space code 0b10, whose PCI and CPU addresses are equal
on every VMM supported). Sizing writes all ones with decoding off and
restores the old value; afterwards memory decoding and bus mastering are
turned on.

The configuration page and the three structures are mapped as device
memory where `Mmio::new` reaches them: the direct-map address on x86_64,
the MMIO window on AArch64. Cloud Hypervisor places 64-bit BARs at the top
of the guest physical address space (below 2^46), which the direct map's
address range covers outside the large-heap window; a BAR that would land
in that window is refused. On x86_64, cell roots copy the kernel's
top-level entries, so the mapping happens in `wire::init`, before the
first cell exists, as for the large-heap window.

### D4. Polled, no MSI-X

As for virtio-mmio (design D4 of `add-virtio-net`), the card is polled and
asks for no interrupts (`VIRTQ_AVAIL_F_NO_INTERRUPT`). MSI-X is never
enabled and INTx is never routed (the I/O APIC and the GIC's SPIs stay
unprogrammed), so neither can disturb the kernel's interrupt model, where
interrupts arrive only while cell code runs or a CPU idles.

### D5. VMM configuration and tests

- QEMU `q35`: `-device virtio-net-pci,netdev=n0,disable-legacy=on,romfile=`
  on the same user network as `microvm` (no boot ROM: nothing for
  SeaBIOS to run).
- Cloud Hypervisor: `--net tap=rhizotap1,mac=…` on a tap device created
  like Firecracker's (`sudo -n`, host 172.16.1.1/24, guest 172.16.1.2), a
  subnet of its own so the two VMMs never share a device; without `sudo`
  it reports why it has no card.
- QEMU `virt`: `virtio-net-pci` in the 4-CPU in-kernel run (`ktest-smp`),
  virtio-mmio in the 1-CPU run (`ktest`), so CI covers both transports on
  AArch64.
- `cargo xtask ci` on x86_64 also runs the in-kernel tests on `q35` and
  checks the card was found on PCI; CI's bench job already runs them on
  Cloud Hypervisor with 4 CPUs, now with the network.

Host tests cover the configuration space (BAR sizing, 64-bit BARs,
assignment from a window, capability parsing and cycles, recognition), the
PCI transport against the simulated device (each register accessed with
its width), the MCFG and the device-tree window.

### Exception: Cloud Hypervisor on AArch64

No CI runner has KVM on AArch64, so Cloud Hypervisor on AArch64 is not
run. Its code path is the one QEMU `virt` takes (device-tree ECAM, the
same driver) except that the VMM assigns the BARs, as on x86_64.
