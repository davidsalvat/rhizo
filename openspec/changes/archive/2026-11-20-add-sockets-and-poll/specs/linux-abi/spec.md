# Spec Delta

## MODIFIED Requirements

### Requirement: Select as a sleep
The kernel SHALL implement `select` (x86_64) and `pselect6` with no
descriptors as a sleep for their timeout, and with descriptors as waits on
them (requirement "Waiting on descriptors").

#### Scenario: Timer select
- **WHEN** a cell calls `select(0, NULL, NULL, NULL, &timeout)`
- **THEN** it sleeps for the timeout and receives 0

## ADDED Requirements

### Requirement: Pipes and event counters
The kernel SHALL implement `pipe2` (and `pipe` on x86_64) and `eventfd2`
(and `eventfd` on x86_64) with Linux semantics: a pipe holds up to 64 KiB,
reading an empty pipe whose write ends are all closed returns 0, writing to
a pipe whose read ends are all closed fails with `EPIPE`; an event counter
adds on `write` and returns and clears its value on `read` (one at a time
with `EFD_SEMAPHORE`).

#### Scenario: Pipe round trip
- **WHEN** a cell writes bytes into a pipe and reads its other end
- **THEN** it reads the same bytes in order

#### Scenario: End of a pipe
- **WHEN** a cell closes the write end of an empty pipe and reads the read end
- **THEN** the read returns 0

### Requirement: Non-blocking descriptors
`O_NONBLOCK` SHALL be settable with `fcntl(F_SETFL)` and with the
`*_NONBLOCK` flags of `pipe2`, `eventfd2`, `socket`, `socketpair` and
`accept4`, and SHALL make a call that would wait fail with `EAGAIN`
(`EINPROGRESS` for `connect`) instead.

#### Scenario: Empty non-blocking pipe
- **WHEN** a cell reads an empty pipe whose read end is non-blocking
- **THEN** it receives `-EAGAIN` at once

### Requirement: Waiting on descriptors
The kernel SHALL implement `poll` (x86_64), `ppoll`, `select`, `pselect6`,
`epoll_create1` (and `epoll_create` on x86_64), `epoll_ctl`, `epoll_wait`
(x86_64) and `epoll_pwait` with Linux semantics for the events `POLLIN`,
`POLLOUT`, `POLLERR`, `POLLHUP`, `POLLRDHUP` and `POLLNVAL` (and their
`EPOLL*` equivalents), level-triggered, with `EPOLLONESHOT`; a call with
nothing ready waits until a descriptor is ready or its timeout ends.

#### Scenario: Timeout with nothing ready
- **WHEN** a cell polls an empty pipe's read end with a 10 ms timeout
- **THEN** the call returns 0 after at least 10 ms

#### Scenario: Ready descriptor
- **WHEN** a cell waits with `epoll_pwait` on a socket that another cell then writes to
- **THEN** the call returns that socket with `EPOLLIN` and the data given at `epoll_ctl`

#### Scenario: Regular file in epoll
- **WHEN** a cell adds a regular file to an `epoll` instance
- **THEN** it receives `-EPERM`

### Requirement: Socket calls
The kernel SHALL implement `socket`, `socketpair`, `bind`, `listen`,
`accept` (x86_64), `accept4`, `connect`, `sendto`, `recvfrom`, `sendmsg`,
`recvmsg`, `shutdown`, `getsockname`, `getpeername`, `setsockopt` and
`getsockopt`, and `read`, `write`, `readv` and `writev` on sockets, for
`AF_INET` (`SOCK_STREAM`, `SOCK_DGRAM`) and `AF_UNIX` (`SOCK_STREAM`), with
Linux error numbers; other families fail with `EAFNOSUPPORT` and other
types with `ESOCKTNOSUPPORT`.

#### Scenario: Same behavior as Linux
- **WHEN** a C program doing a TCP echo over 127.0.0.1 with `poll` and `epoll`, a pipe, an event counter, UDP datagrams and a Unix socket runs as a cell and on Linux
- **THEN** both print the same output and exit with the same status

#### Scenario: No listener
- **WHEN** a cell connects to a loopback port nobody listens on
- **THEN** it receives `-ECONNREFUSED`

### Requirement: Socket options
`setsockopt` SHALL accept `SO_REUSEADDR`, `SO_REUSEPORT`, `SO_KEEPALIVE`,
`SO_RCVBUF`, `SO_SNDBUF`, `SO_RCVTIMEO`, `SO_SNDTIMEO`, `SO_LINGER`,
`SO_BROADCAST`, `TCP_NODELAY`, `TCP_KEEPIDLE`, `TCP_KEEPINTVL` and
`TCP_KEEPCNT`; `getsockopt` SHALL report them and `SO_ERROR`, `SO_TYPE`,
`SO_DOMAIN` and `SO_ACCEPTCONN`. `SO_RCVTIMEO` and `SO_SNDTIMEO` SHALL
bound a blocking wait, which then fails with `EAGAIN`.

#### Scenario: Receive timeout
- **WHEN** a cell sets a 10 ms `SO_RCVTIMEO` and receives on a socket with no data
- **THEN** the call fails with `-EAGAIN` after at least 10 ms
