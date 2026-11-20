# Design

## Decisions

### D1. Private objects in the cell's tables, sockets in the kernel's

A pipe, an event counter or an `epoll` instance is reachable only through
the descriptors of one cell: it lives in the cell's `rhizo-fs` tables, next
to its files, as plain data. It is therefore carried by snapshots and copied
copy-on-write by forks with no new mechanism, exactly like an open file.

A socket must be reachable from other cells (a client in one cell, a server
in another), so sockets live in one kernel-wide table, `rhizo_net::Net`, and
a descriptor holds a handle to one. A handle is counted once per cell file
description that refers to it; the socket closes when the last one goes. A
Unix socket's name is a file of type socket in the cell's tree (as on Linux:
`bind` creates it, `stat` shows it, `unlink` removes it) holding a name
number that `Net` maps to the listening socket; a name whose socket closed
refuses connections (`ECONNREFUSED`).

| Option | Cross-cell | Snapshot / fork |
|---|---|---|
| **Private objects in `rhizo-fs`, sockets in `Net` (chosen)** | yes | private objects free; sockets by rule (D4) |
| Everything in `Net` | yes | pipes would need the socket rules too |
| Everything in the cell | no: two cells could never talk | free |

### D2. A loopback transport, not a TCP/IP stack (yet)

On 127.0.0.0/8 a TCP connection is a pair of byte queues joined in the
kernel: `connect` finds the listener, queues the accepted end and returns;
`send` appends to the peer's queue; a close is an end of stream for the peer.
UDP is a queue of datagrams per bound port. There are no segments,
checksums, windows or retransmissions: nothing between two cells of one
kernel can be lost, so they would only cost time (rule 7).

The stack for the network card was evaluated now so the two fit:

| Option | License | For loopback | For the wire |
|---|---|---|---|
| `smoltcp` 0.12 | 0BSD (permissive) | headers, checksums and segmentation per packet, polled: slower | complete (TCP, UDP, ARP, DHCP), `no_std`, used by many kernels |
| Own TCP/IP | MIT (ours) | — | thousands of lines of retransmission, windows, congestion control to get right |
| **Loopback pairs now, `smoltcp` for the wire in `add-virtio-net` (chosen)** | — | one copy in, one out | `smoltcp` behind `Net` |

This change adds no dependency. `Net` keeps the socket API (states,
addresses, readiness, options) independent of the transport so that
`add-virtio-net` routes non-loopback addresses to `smoltcp` sockets.

### D3. Blocking by restarting the call

A call that cannot complete (a read on an empty pipe or socket in blocking
mode, `accept` with no connection, `poll` with nothing ready and a timeout)
returns `Outcome::Block(deadline)` with the program counter moved back onto
the system-call instruction (as Linux does for a restarted call). The kernel
parks the cell in the scheduler's waiting set; it uses no CPU. `Net` counts
every change another cell could be waiting for (data queued, a connection
queued, a close); after each scheduling step that changed the count, every
waiting cell becomes runnable and re-executes its call, which completes or
blocks again. A deadline wakes it too; the kernel keeps the call's deadline
across re-executions so a timeout is not extended.

Because the cell's registers always describe a call that has not run, a
blocked cell needs nothing new in its snapshot: suspended or forked, it
re-executes the call (with its whole timeout, a documented difference from
Linux's restart blocks). Waking every waiting cell on a change is linear in
waiting cells; per-object wait queues would wake fewer, at the cost of
pointers between objects and cells that snapshots cannot hold. Revisit when
a benchmark shows it.

A blocking write of more than the room left writes what fits and returns
that count (Linux would wait for the rest); stream buffers are 256 KiB and
pipes 64 KiB, so programs that write less than that see Linux's behavior.

### D4. Suspend, resume and fork

| What | Suspend → resume | Fork (child) |
|---|---|---|
| pipe, event counter, `epoll` | carried whole (contents, ends, interest list) | copied (copy-on-write) |
| file of type socket (Unix name) | kept; refuses connections unless the cell's own listener is in the snapshot | kept, names the same listener |
| socket not bound | recreated | copied |
| bound or listening socket | re-bound (and listening) at its address; if the address is taken, reset | shared with the parent (both accept from one queue, as after a Linux `fork`) |
| connected stream socket, or connecting | reset: `ECONNRESET` on the next call, `POLLERR`/`POLLHUP` | reset in the child; the parent keeps it |
| queued data, unaccepted connections | dropped (connections reset for their peers) | listener queue shared |

Two cells cannot share one byte stream without coordinating who reads what,
and a connection's peer cannot be suspended with it, so connections are
reset, as CRIU does by default (without `--tcp-established`). Everything a
cell owns alone is carried. The snapshot holds one record per socket
(family, type, state, address, options, Unix name number); the descriptors
refer to records by index, and resume maps them to new handles. Snapshot
format version 4 adds that section after the files.

### D5. Readiness

`poll`, `select` and `epoll` ask each descriptor for its events: files and
directories are always readable and writable, the console reads as empty,
pipes, counters and sockets report their queues, ends and errors. `epoll`
keeps an interest list (descriptor, events, data) and is level-triggered;
`EPOLLET` is accepted and reported level-triggered (a program using it
drains to `EAGAIN` before waiting, so it behaves the same), `EPOLLONESHOT`
disables the entry after one report. Closing a descriptor removes it from
the cell's `epoll` lists. `epoll_ctl` on a regular file fails with `EPERM`,
as on Linux. Signal masks of `ppoll`, `pselect6` and `epoll_pwait` are
ignored (no signals are delivered yet).

### D6. Loopback addresses and ports

Sockets bind to 127.0.0.0/8 or `INADDR_ANY` (both mean loopback here); other
addresses fail with `EADDRNOTAVAIL`, and connecting or sending to them with
`ENETUNREACH` until `add-virtio-net`. Port 0 picks the next free ephemeral
port from 32768–60999 in order (no randomness in core logic, rule 2).
`SO_REUSEADDR` lets a listener re-bind a port whose connections remain; two
listeners on one port fail with `EADDRINUSE`. A connection to a port with no
listener fails with `ECONNREFUSED`; one to a listener with a full backlog
too. A non-blocking `connect` returns `EINPROGRESS` and the socket is
writable at once, as on Linux over loopback.

### D7. Benchmarks

`cell.pipe_rtt_ns`: one cell writes a byte into a pipe and reads it back,
10 000 times; time per round trip (two system calls). `cell.loopback_rtt_us`:
two cells exchange a 64-byte message over TCP on 127.0.0.1, 2 000 times,
blocking reads; time per round trip (two blocks, two wakes, two switches).
Both are robust figures (minimum over rounds of the median, as the file
metrics).

### D8. A 2 MiB copy-on-write without a free 2 MiB block

A copy-on-write write to a shared 2 MiB page needed a whole free 2 MiB
block, and the cell was killed without one. With the security and thread
tests merged before this change, the MicroPython in-kernel test found too
few: the memory-pressure test leaves the heap's size-class pages (kept for
good) spread over nearly every block, about four per block, and a
MicroPython fork faulted, depending on scheduling. The page-fault path
already falls back to 4 KiB pages; copy-on-write now does too: with no
free 2 MiB block it copies the page into 512 private 4 KiB frames under a
new table, so the write succeeds while 4 KiB frames remain. The usual
case (a free block) is unchanged.
