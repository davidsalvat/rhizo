## ADDED Requirements

### Requirement: Receive buffers
The network card driver SHALL keep receive buffers offered to the device
that add up to at least 65,562 bytes (a 64 KiB frame and its virtio-net
header) while frames wait for the kernel to take them, so that a device
that hands over a frame only when that much is offered (Firecracker) still
delivers bursts of frames.

#### Scenario: Device that waits for 64 KiB
- **WHEN** a device delivers frames only while the offered receive buffers add up to 65,562 bytes, and a burst arrives before the kernel polls the card
- **THEN** at least 16 frames of the burst are delivered, and the kernel receives them all

#### Scenario: Echo from the host on Firecracker
- **WHEN** the kernel boots on Firecracker with a tap device, `ip=172.16.0.2/24,172.16.0.1` and a TCP echo server on the host's tap address
- **THEN** a cell's messages to the echo server come back and the cell exits with status 0, with one CPU and with four
