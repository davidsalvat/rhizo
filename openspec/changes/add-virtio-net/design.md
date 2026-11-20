# Design

## Decisions

### D1. virtio-mmio only, modern

QEMU `virt`, QEMU `microvm` and Firecracker attach virtio devices through
virtio-mmio; Firecracker implements only the modern interface (version 2),
and QEMU offers it with `-global virtio-mmio.force-legacy=false`, which
`xtask` passes. One transport covers three VMMs on both architectures.
Cloud Hypervisor and QEMU `q35` expose virtio only on PCI: a virtio-pci
transport needs PCI enumeration, BARs and capability parsing (and MSI-X
for interrupts), a driver of its own; until one is written they run
without a card, and the network tests and metric are reported as skipped
there with that reason.

| Option | VMMs covered | Work |
|---|---|---|
| **virtio-mmio v2 (chosen)** | QEMU virt, QEMU microvm, Firecracker | one transport |
| + legacy virtio-mmio v1 | the same (QEMU only) | a second queue layout for nothing |
| + virtio-pci | Cloud Hypervisor, q35 | PCI, BARs, capabilities: later |

### D2. `smoltcp` behind `Net`, loopback unchanged

`smoltcp` 0.14 (license 0BSD, `no_std`, `alloc`, features: Ethernet,
IPv4, TCP, UDP, DHCPv4; its dependencies are 0BSD, MIT or Apache-2.0, or
MIT-or-Unlicense) is the TCP/IP stack on the card. It was weighed against
writing TCP (retransmission, windows, congestion control, ARP, DHCP: a
large, risky body of code) and wins on rule 10: it is the smallest correct
option. `deny.toml` allows 0BSD for it. Loopback keeps the byte-queue pairs
of `add-sockets-and-poll` (faster: no packets at all).

A socket whose destination is not on 127.0.0.0/8 uses a `smoltcp` socket:
`connect` starts the handshake (`EINPROGRESS` non-blocking; a blocking call
waits and is made again, design D3 of `add-sockets-and-poll`), `send` and
`recv` move bytes through `smoltcp`'s buffers (64 KiB each way), a close
sends a FIN and the socket is dropped from `smoltcp` once closed. UDP
sockets get a `smoltcp` socket on the same port the first time they send to
the wire. Readiness, `SO_ERROR`, shutdown and the suspend/fork rules
(connections reset) are those of loopback sockets.

### D3. Hardware behind two traits

`rhizo-hal` gains `DeviceRegisters` (32-bit register reads and writes and
an ordering barrier) and `NetLink` (send a frame, take the next received
frame, the MAC address). `rhizo-virtio` implements `NetLink` for a virtio
network device on any `DeviceRegisters` and `PhysicalMemory`, so the
driver's queue logic runs on the host against a simulated device. The
kernel's only hardware code is the register accessor (volatile accesses,
`dmb`/`mfence`) and the discovery of devices, in `arch/*/platform.rs`.

### D4. Polled, not interrupt-driven

The kernel polls the card and `smoltcp` once per scheduling step, and,
while no cell can run, at least every 100 µs when sockets use the wire
(otherwise it idles as before). Interrupts from the card would need the
GIC (AArch64) and the I/O APIC (x86_64) to route device lines, which the
kernel does not drive yet; polling keeps the change on one interface and
costs only idle time. Revisit with a benchmark of idle CPU.

### D5. Addresses

`ip=dhcp` runs `smoltcp`'s DHCP client; `ip=ADDR/PREFIX[,GATEWAY]` sets a
static address (Firecracker has no DHCP server). No `ip=` option: no
address, so connections to the wire fail with `ENETUNREACH` until one is
configured. Binding to the card's address is allowed; other non-loopback
addresses stay `EADDRNOTAVAIL`. Ephemeral ports come from the same range
as loopback sockets.

### D6. Tests and metric

`xtask` starts a TCP echo server on the host for every QEMU run and passes
its address as `echo=HOST:PORT` (QEMU's user network reaches the host's
127.0.0.1 at 10.0.2.2); the in-kernel test runs a cell that connects to it,
sends messages and checks the echoes. Firecracker gets a tap device
(`sudo -n ip tuntap`), its host end 172.16.0.1/24 and the guest
172.16.0.2/24; without `sudo` the run reports why it has no card.
`net.tcp_rtt_us` is a cell's 64-byte round trip to that echo server.
