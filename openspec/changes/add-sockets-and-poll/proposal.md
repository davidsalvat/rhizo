# Proposal

## Why

Agents call APIs over the network and wait on several descriptors at once:
an HTTP client, a child's pipe, a timer. Cells have files only: `select`
and `pselect6` work only without descriptors, and there are no pipes and no
sockets. Before a network card (the next change, `add-virtio-net`), cells
need the descriptors that wait — pipes, event counters, `poll`, `select`,
`epoll` — and sockets, so that two cells of the same kernel can talk over
TCP and UDP on 127.0.0.1 or over Unix sockets. Rule 6 still holds: what a
cell has open must survive a suspend, a resume and a fork in a defined way.

## What Changes

- Waiting descriptors: `pipe2` (and `pipe` on x86_64), `eventfd2`,
  `poll`/`ppoll`, `select`/`pselect6` with descriptors,
  `epoll_create1`/`epoll_ctl`/`epoll_wait`/`epoll_pwait`, `O_NONBLOCK`
  through `fcntl` and the `*_NONBLOCK` flags. A call that must wait blocks
  the cell without using the CPU, with its timeout, through the scheduler
  (time passed in, never read).
- Sockets: `socket`, `socketpair`, `bind`, `listen`, `accept`/`accept4`,
  `connect`, `sendto`/`recvfrom`, `sendmsg`/`recvmsg`, `shutdown`,
  `getsockname`/`getpeername`, `setsockopt`/`getsockopt` (common options
  accepted), with `read`/`write` on sockets: `AF_UNIX` stream sockets named
  by files in the cell's tree, and `AF_INET` TCP and UDP on the loopback
  network (127.0.0.0/8), shared by every cell of the kernel.
- New crate `rhizo-net`: the kernel's sockets as plain data (no hardware,
  no clock), host-tested; no third-party dependency (design D2 explains
  why `smoltcp` comes with the network card, not here).
- Suspend, resume and fork: pipes, event counters and `epoll` instances are
  carried whole; listening and datagram sockets are re-bound on resume and
  shared with forks; established stream connections are reset
  (`ECONNRESET`), as CRIU does without `tcp-established`. Snapshot format
  version 4 has a sockets section.
- Tests: host tests in every crate touched, in-kernel tests across cells, and
  a real C program (static glibc, `cells/c/libc_net.c`) doing a TCP echo
  over loopback with `poll`, `epoll`, a pipe, an event counter, UDP and a
  Unix socket, checked against Linux natively (differential check).
- Benchmarks: `cell.pipe_rtt_ns` and `cell.loopback_rtt_us`.

### Out of scope

A network card and addresses other than loopback (`add-virtio-net`), IPv6,
`AF_UNIX` datagram sockets and descriptor passing, signals interrupting a
wait, `epoll` edge triggering (accepted, reported level-triggered), urgent
data, `SO_LINGER` behavior, socket buffers larger than fixed bounds.

## Capabilities

### New Capabilities

- `networking`: sockets between cells, waiting on descriptors, and what
  happens to them when cells are suspended, resumed and forked.

### Modified Capabilities

- `linux-abi`: pipes, event counters, `poll`, `select`, `epoll` and socket
  calls; `select` with descriptors.
- `scheduling`: cells blocked on descriptors.
- `snapshots`: pipes and sockets in snapshots (format version 4).
- `fork`: pipes and sockets of a forked cell.
- `benchmarks`: pipe and loopback round-trip metrics.

## Impact

`crates/rhizo-net` (new), `crates/rhizo-fs` (pipes, event counters, `epoll`
instances, socket names, socket descriptors), `crates/rhizo-abi`,
`crates/rhizo-sched` (waiting cells), `crates/rhizo-snapshot` (version 4),
`kernel/src/cell.rs`, `cells/`, `xtask/src/bench.rs`, `xtask/src/deps.rs`,
`AGENTS.md`, `bench/README.md`. The release kernel image grows (new system
calls and the socket layer); `image.size_bytes` is re-baselined in the pull
request with that justification.
