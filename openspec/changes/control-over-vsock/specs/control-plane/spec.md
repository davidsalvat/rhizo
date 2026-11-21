## MODIFIED Requirements

### Requirement: Control channel on every VMM
The control channel SHALL be the VM's virtio socket device where the VM
has one, and the VM's serial line otherwise, on QEMU, Firecracker and
Cloud Hypervisor and on both architectures. The kernel's console text
SHALL stay on the serial line, and a cell's output SHALL reach the host
only in the protocol's frames.

#### Scenario: Boot on the default VMM
- **WHEN** `rhizo up` boots the control kernel on QEMU (`microvm` on x86_64, `virt` on AArch64)
- **THEN** the host receives the kernel's hello on the VMM's standard output, after its console text

#### Scenario: Without a socket device
- **WHEN** the control kernel runs on QEMU and the host's `/dev/vhost-vsock` is missing or unusable
- **THEN** requests and replies take the serial line, and the `control` test step passes as before

## ADDED Requirements

### Requirement: Control over vsock
A control kernel SHALL say hello on its serial line once booted, listen on
vsock port 1024 of its socket device, and say hello again on each
connection the host makes there, after which its frames SHALL take that
connection until it ends.

#### Scenario: Second hello
- **WHEN** the host connects to port 1024 of a booted control kernel's socket device
- **THEN** it receives a hello on the connection, and its requests sent there are answered there

### Requirement: Socket device on every VMM
The host SHALL give a control kernel's VM a socket device on Firecracker
and Cloud Hypervisor, and on QEMU when the host's `/dev/vhost-vsock` is
usable, and drive the kernel through it.

#### Scenario: Cloud Hypervisor
- **WHEN** the `control` test step runs on Cloud Hypervisor, whose serial line takes no input from a pipe
- **THEN** the kernel answers every request, over its socket device

### Requirement: One vsock connection
The control kernel's vsock stream SHALL hold one connection at a time,
made by a request to its port, and answer any other packet with a reset,
except a reset; it SHALL reset its connection when the peer sends a
shutdown, another socket type, or a length that is not its payload's, and
ignore unknown operations.

#### Scenario: Hostile packets
- **WHEN** arbitrary packets (any addresses, lengths, credit values and operations) reach the stream between its writes and reads
- **THEN** nothing panics, and every packet it sends carries a payload of its stated length

### Requirement: Bounded vsock buffers
The vsock stream SHALL never send more than the credit its peer gave, never
hold more unread bytes than the receive buffer it advertises (256 KiB),
and reset its connection when the peer sends beyond its credit or reads
too little to take 4 MiB of pending output. Nothing a device sends SHALL
make the driver read or write outside its buffers.

#### Scenario: Credit overflow
- **WHEN** the peer gives any credit, counters that overflow included, while the stream has bytes to send
- **THEN** the stream sends at most the peer's buffer less what it sent and the peer has not read

#### Scenario: Hostile device
- **WHEN** the device reports a used buffer shorter than a header, a length beyond the buffer, or a descriptor out of range
- **THEN** the driver skips or clips it and its buffers keep working
