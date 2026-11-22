## ADDED Requirements

### Requirement: Control channel secret
A control kernel with a socket device SHALL draw a 256-bit secret at boot
from its generator seeded from the hardware, and give it only on its
serial line, before its hello, as the console line `vsock: secret `
followed by 64 hex digits; never on its command line. Without a hardware
random source it SHALL not listen on vsock, and its frames SHALL stay on
the serial line.

#### Scenario: Secret before the hello
- **WHEN** a control kernel with a socket device boots on a CPU with a hardware random source
- **THEN** its serial line carries the secret line, then the hello

### Requirement: Authenticated vsock connections
The control kernel SHALL serve a vsock connection only once its first 32
bytes are the secret, compared in constant time once all 32 have come.
Before that it SHALL neither read nor answer the connection's bytes, nor
send it any frame. Every new connection, after a drop too, SHALL present
the secret again.

#### Scenario: A stranger's request
- **WHEN** a connection to port 1024 sends frames, or a wrong secret followed by a power-off request, as its first bytes
- **THEN** it is reset without receiving a byte, the kernel does not power off, and the host's connection that presents the secret is then greeted and answered

#### Scenario: A split secret
- **WHEN** a connection sends the secret split over many packets, followed by a frame
- **THEN** it is served, and the frame is read as its first

#### Scenario: A reconnect after a drop
- **WHEN** the host's served connection ends and a new connection is made
- **THEN** the new connection is not served until it presents the secret

### Requirement: Refused vsock connections
The control kernel SHALL reset a connection whose first 32 bytes are not
the secret, that has not sent them within 2 seconds, or that has not sent
them when another peer asks to connect; it SHALL then keep listening, and
count and report each refusal on its console.

#### Scenario: A slow connection
- **WHEN** a connection sends part of the secret and nothing more for 2 seconds
- **THEN** it is reset, the refusal is reported, and a later connection with the secret is served

#### Scenario: A stranger connected first
- **WHEN** a stranger's connection holds the stream without the secret and the host connects
- **THEN** the stranger's connection is reset and the host's is served once it presents the secret

### Requirement: The host presents the secret
The host's tools (`xtask` and `rhizo up`) SHALL read the control kernel's
secret from its serial line, keep it out of the console text they show or
store, persist it nowhere, and send it as the first bytes of every vsock
connection to the kernel, on every VMM; with no secret given they SHALL
drive the kernel over its serial line. `rhizo up` SHALL keep its state
directory at mode `0700` and its control socket at mode `0600`.

#### Scenario: Console log
- **WHEN** `rhizo up` boots a control kernel with a socket device
- **THEN** its `console.log` holds no secret line, its state directory has mode `0700`, and its requests are answered over the vsock connection
