# Spec Delta

## ADDED Requirements

### Requirement: Descriptor ioctls
`ioctl` SHALL implement `FIONBIO` (set or clear a descriptor's
non-blocking mode, as `fcntl(F_SETFL)` with `O_NONBLOCK` does), `FIOCLEX`
and `FIONCLEX` (set or clear close-on-exec) on any descriptor, with `EBADF`
for a descriptor that is not open and `EFAULT` for an unreadable argument;
any other request SHALL return `ENOTTY`.

#### Scenario: Socket with a timeout
- **WHEN** CPython creates a socket with a timeout (`socket.create_connection(..., timeout=...)`), which switches it to non-blocking mode with `FIONBIO`
- **THEN** the call succeeds and `fcntl(F_GETFL)` reports `O_NONBLOCK`

#### Scenario: Close-on-exec
- **WHEN** a cell calls `FIOCLEX` then `FIONCLEX` on a descriptor
- **THEN** `fcntl(F_GETFD)` reports `FD_CLOEXEC` after the first and not after the second

#### Scenario: Other requests
- **WHEN** a cell asks for a terminal's attributes with `TCGETS`
- **THEN** the call returns `-ENOTTY`
