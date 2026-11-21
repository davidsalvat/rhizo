## MODIFIED Requirements

### Requirement: Control over vsock
A control kernel SHALL say hello on its serial line once booted, listen on
vsock port 1024 of its socket device, and say hello again on each
connection the host makes there, after which its frames SHALL take that
connection until it ends. A connection that replaces another between two
looks of the kernel SHALL be greeted in the same way, and no byte of one
connection SHALL be read as part of another's frames.

#### Scenario: Second hello
- **WHEN** the host connects to port 1024 of a booted control kernel's socket device
- **THEN** it receives a hello on the connection, and its requests sent there are answered there

#### Scenario: A reconnect between two looks
- **WHEN** the host resets its connection in the middle of a frame and connects again, and the kernel receives the reset, the new connection and its first bytes together
- **THEN** the new connection has a new number, the old connection's partial frame is dropped, and the new connection's bytes are read only as its own
