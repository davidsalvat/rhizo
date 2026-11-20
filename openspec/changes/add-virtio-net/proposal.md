# Proposal

## Why

`add-sockets-and-poll` lets cells of one kernel talk over loopback. Agents
call APIs outside their machine: cells need a network card and a TCP/IP
stack, with the same socket calls, so that a static program that opens a TCP
connection on Linux does the same in a cell.

## What Changes

- A virtio-net driver over the virtio-mmio transport (modern, version 2):
  QEMU `virt` (AArch64), QEMU `microvm` (x86_64) and Firecracker (both
  architectures). New crate `rhizo-virtio` (transport, split virtqueues,
  the network device) works through two new `rhizo-hal` interfaces,
  device registers and a network link, and is tested on the host against a
  simulated device. The kernel finds the device in the device tree
  (AArch64) or in the `virtio_mmio.device=` command-line option the VMM
  adds (x86_64), and maps its registers behind the `arch` interface.
- `rhizo-net` gets a wire: `smoltcp` (0BSD) on the card for every address
  that is not loopback. Cells connect out over TCP and send and receive UDP
  datagrams with the socket calls they already have; blocking, `poll`,
  `epoll`, timeouts and suspend/fork rules (connections reset) apply
  unchanged.
- Address configuration from the kernel command line: `ip=dhcp` or
  `ip=ADDR/PREFIX[,GATEWAY]`.
- `xtask` gives QEMU runs a user-mode network (slirp) and a TCP echo server
  on the host, and Firecracker a tap device when `sudo` allows it
  (skipped with the reason otherwise); an in-kernel test reaches the host's
  echo server from a cell.
- Benchmark `net.tcp_rtt_us`: a cell's round trip to the host's echo
  server.

### Out of scope

virtio-pci (Cloud Hypervisor and QEMU `q35` expose virtio devices only on
PCI: they keep running without a card), incoming connections from the
wire (listeners stay loopback-only), IPv6, DNS resolution in the kernel
(a cell's libc resolves through UDP), interrupts from the card (polled).

## Capabilities

### Modified Capabilities

- `networking`: a network card, addresses, connections to the outside.
- `benchmarks`: network round trip to the host.

## Impact

`crates/rhizo-hal` (two traits), `crates/rhizo-virtio` (new),
`crates/rhizo-net` (`smoltcp` dependency, wire), `crates/rhizo-core`
(`ip=` and `echo=` options), `kernel` (device discovery and mapping on both
architectures, polling the card in the scheduler loop), `cells/`,
`xtask` (network for VMMs, host echo server, metric), `deny.toml` (0BSD),
AGENTS.md code map. The release kernel image grows by the stack in builds
that run cells only.
