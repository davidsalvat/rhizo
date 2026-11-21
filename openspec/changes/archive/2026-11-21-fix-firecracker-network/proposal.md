# Proposal

## Why

On Firecracker the network card sends but never receives: the guest gets
no answer from the host's echo server over the tap device, so
`net.tcp_rtt_us/firecracker` is skipped ("no answer from the echo server in
10 s") and the in-kernel tests run there without the host network.

The cause is the size of the receive queue. Firecracker (v1.15,
`devices/virtio/net/device.rs`, `read_from_mmds_or_tap`) reads a frame from
its tap only while the receive buffers the driver offered add up to at
least `MAX_BUFFER_SIZE`, 65,562 bytes (a 64 KiB frame and its virtio-net
header), whatever features were negotiated. The driver offers 32 buffers of
2,048 bytes: 65,536 bytes, 26 short. Firecracker counts each frame from the
tap as `no_rx_avail_buffer` and leaves it there; the guest's ARP request
reaches the host, the host's reply never reaches the guest. QEMU hands a
frame over as soon as one buffer is free, so it was not affected.

## What Changes

- The virtio-net driver's queues get 64 entries instead of 32: the receive
  queue offers 128 KiB, above Firecracker's minimum while up to 31 frames
  wait for the kernel. Buffers stay 2,048 bytes; the card takes 66 frames
  instead of 34.
- `xtask`: the 4-CPU in-kernel tests on Firecracker run with the host
  network; CI also runs the one-CPU in-kernel tests on Firecracker (both
  include `a_cell_reaches_the_host`). `net.tcp_rtt_us/firecracker` is
  measured.

### Out of scope

`VIRTIO_NET_F_MRG_RXBUF` and offloads (Firecracker's minimum applies with
them too), interrupts from the card, virtio-pci.

## Capabilities

### Modified Capabilities

- `networking`: receive buffers enough for every supported VMM; cells reach
  the host on Firecracker.

## Impact

`crates/rhizo-virtio` (queue size, host test against a device that delivers
as Firecracker does), `xtask/src/main.rs`, `.github/workflows/ci.yml`.
Memory: 128 KiB more for the card's buffers.
