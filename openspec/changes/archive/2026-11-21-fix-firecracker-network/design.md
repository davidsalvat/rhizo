# Design

## Context

The driver (`rhizo-virtio`) polls a virtio-mmio network card with one
receive and one transmit split virtqueue of `QUEUE_SIZE` descriptors, each
pointing at a 2,048-byte buffer. Every queue sits in one 4 KiB frame:
descriptors at 0, available ring at 1,024, used ring at 2,048.

Firecracker's network device keeps the receive descriptors it has parsed and
reads the tap only while their total length is at least 65,562 bytes
(`MAX_BUFFER_SIZE`); otherwise it counts `no_rx_avail_buffer` and waits for
the next queue notification. 32 buffers of 2,048 bytes never reach it.

## Decisions

### D1. 64 descriptors per queue

64 buffers of 2,048 bytes offer 131,072 bytes; Firecracker keeps reading
while at least 33 are free, so up to 31 frames can wait between two polls
of the card. 64 is the most the queue's one-frame layout holds (64
descriptors of 16 bytes fill the 1,024 bytes before the available ring);
compile-time assertions check the layout and the minimum. QEMU and
Firecracker accept queues of up to 256 entries.

Alternatives that lost:

- **33 descriptors**: the minimum, but after one frame the offered bytes
  fall below 65,562 again, and Firecracker stops after every frame until
  the kernel gives the buffer back: a frame per notification.
- **Larger buffers (4,096 bytes, 32 descriptors)**: the same 128 KiB, but
  half of every buffer is never written (frames are at most 1,514 bytes
  without offloads) and burst capacity halves to 15 frames.
- **`VIRTIO_NET_F_MRG_RXBUF`**: Firecracker applies the same 65,562-byte
  minimum with it; it changes nothing here and adds header handling.
- **Static address and routing on the host**: already in place (`ip=` on
  the command line, tap at 172.16.0.1/24); the capture shows the host
  answering ARP, so the host side was not the cause.

### D2. Tests

A host test drives the card with a simulated device that delivers a frame
only while the offered receive buffers reach 65,562 bytes, as Firecracker
does; with 32 descriptors it receives nothing. On Firecracker the in-kernel
tests (`ktest`, `ktest-smp`) run with the tap and the echo server, and the
benchmark measures `net.tcp_rtt_us/firecracker`.

## Risks

- 128 KiB more memory for the card, taken once at boot; the probe zeroes 32
  more frames (microseconds, only when a card is present).
