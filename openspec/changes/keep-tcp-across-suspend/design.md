# Design

## Context

`rhizo-net`'s `Net` holds every socket of the kernel. A loopback (or Unix)
connection is two sockets joined by handle, each with the queue of bytes
its peer sent; a connection through the card is a `smoltcp` TCP socket in
the kernel's interface. Until now, suspending a cell released its sockets
(the connection closed for the peer) and the snapshot kept a `Reset`
record (design D4 of `add-sockets-and-poll`).

## Goals / Non-Goals

Goals: within one kernel, a suspend and resume is invisible to a cell's
connections and to their peers (apart from the time it takes); everything
else (another boot, a second resume, a fork) fails cleanly and at once.
Non-goals: moving a connection to another kernel; keeping UDP queues
across a suspend.

## Decisions

### D1. Park the connection in the kernel, do not serialize it

| Option | Correct within one kernel | Across kernels | Cost |
|---|---|---|---|
| **Park: the socket stays in `Net`, the snapshot holds a key (chosen)** | yes: the peer and the card keep talking to a live socket | no (reset) | one key per connection; no copy |
| Serialize: copy sequence numbers, windows and buffers into the snapshot, rebuild a socket on resume | only if nothing happens while suspended; it does: the peer keeps sending, ACKs and retransmissions are due, timers run | only with the peer's cooperation (TCP repair, same addresses) | a `smoltcp` fork (it exposes no way to set sequence numbers or unacknowledged data), a copy of up to 128 KiB per connection both ways |
| Reset (before) | no | no | none |

A TCP connection is shared state between two ends that keep running. While
a cell is suspended its peer still sends, still expects ACKs, and the
cell's unacknowledged bytes must still be retransmitted; a copy of the
state taken at suspend is stale by the time of the resume, and resuming
from it would send sequence numbers the peer has moved past. Keeping the
socket where it is — in `Net`, for loopback and Unix pairs, and in the
`smoltcp` interface, which the kernel keeps polling (`Net::wire_busy` stays
true) — is the only option that is correct without the peer's help, and it
is also the simplest and fastest: suspend and resume copy nothing. So the
sequence numbers, windows, buffers and options of a parked connection are
*recorded in the kernel*, with the connection; the snapshot records where
to find it (D2). That is what "the snapshot records enough socket state"
means for resuming in the same kernel: the record is a reference, and the
state behind it is kept exact and live instead of frozen and stale.

`Net::export` parks a connected or connecting stream socket (`Connected`,
`Wire`) and a listener: it takes one more reference to it, and puts it in
a table `parked: key → handle`. The cell's own references are released as
before when it is suspended, so the parked reference keeps the socket
open. `Net::import` of a parked record looks the key up; found, the entry
leaves the table and its reference becomes the resumed cell's. A cell
whose two ends of one connection are both parked gets both back, still
joined. A file description refers to its socket once, so one record per
description is exactly one reference per description.

Listeners are parked too. Releasing a listener at suspend (as before)
resets every connection in its backlog, so a client that connected just
before the server was suspended, or while it was, would see a reset or a
refusal: a server suspended between a client's `connect` and its own
`accept` breaks that client (the in-kernel test that suspends a server
after every scheduling step found exactly this). Parked, the listener keeps
its port and its backlog, and the server accepts after its resume what
arrived meanwhile, like a busy server. A listener shared with a fork keeps
accepting there meanwhile. A re-attached Unix listener keeps its name
number, so the cell's socket file still reaches it.

### D2. The parked record: boot identity and key

A parked record carries `Park { boot, key }`. `key` counts up in each `Net`; `boot` is
a 64-bit value the kernel passes to `Net::set_boot` when it starts (its
cycle counter, the same source as `smoltcp`'s seed; no randomness is read
in `rhizo-net`). A record from another boot does not match `boot` and is
not re-attached — even if its key happens to be in use in this boot: the
record's own state then says what comes back (a connection reset, a
listener bound and listening again, reset if its address is taken). A
key that is not in the table (already re-attached by an earlier resume of
a copy of the snapshot, or discarded) is treated the same way. Two cells
can never share one byte stream (design D4 of `add-sockets-and-poll`), so
"first resume wins" is the only consistent rule.

The record keeps family, type, state, address and options as before, so
the fallback is exactly the old behaviour; a parked connection's record
says `Reset` without an address. Snapshot format version 6 adds, after the
backlog, `parked: u8` and, when it is 1, `boot: u64` and `key: u64`; a
parked record must be a listener or a reset stream without an address,
anything else is refused. Snapshot bytes are produced by the kernel's host: a forged
snapshot with a guessed key could take a parked connection of another
suspended cell of the same boot; the host can already do anything to
every cell, so this needs no more than the boot identity.

### D3. While suspended: buffers, windows, closes

Nothing changes for the parked socket's peer except that nobody reads:

- loopback and Unix: the peer's writes fill the parked socket's queue up to
  `STREAM_BUF` (256 KiB), then fail with `EAGAIN` (the peer blocks), as
  with any slow reader;
- the card: `smoltcp` keeps receiving into the socket's 64 KiB buffer and
  acknowledging; the window it advertises shrinks to zero, and the remote
  host waits (zero-window probes are answered). The cell's unacknowledged
  bytes are retransmitted by `smoltcp`'s timers, which run each time the
  kernel polls the card. The connection's 20 s timeout applies only to
  data the *peer* does not acknowledge, as for a running cell;
- a peer that closes or resets meanwhile is recorded by the socket (end of
  stream, or reset) and reported to the cell after the resume as usual.

A parked socket costs nothing but its buffers. A snapshot that is never
resumed would hold its sockets forever: `cell::close_parked(snapshot)`
releases them (`Net::unpark`): the peer of a connection sees the end of
the stream, connections waiting on a listener are reset.

### D4. Fork and templates

The child cannot share the byte stream with its parent (who would read
which bytes?), and it cannot get its own connection (the peer knows one).
So the parent keeps the connection and the child's copy is reset, as
before; this change states and tests the exact sequence the child sees:
`ECONNRESET` on the first call that reports errors, then end of stream for
reads and `EPIPE` for writes (Linux after a received RST; the kernel has no
signals, so `EPIPE` is returned as with `SIGPIPE` ignored), `ENOTCONN` for
`getpeername`. A listener is not a byte stream: each accepted connection
belongs to whoever accepts it, so parent and child share it (one queue, as
after a Linux `fork`), which is what pre-forking servers and templates
need (scenario "Forked servers"). Templates follow the fork rule: the
frozen cell keeps its connections (they stay open until the template is
dropped), cells spawned from it get reset copies.

### D5. Metric

`cell.tcp_resume_us`: a `loopback` client (`twice` mode) makes one round
trip with a `loopback` server, yields, and is suspended right after the
yield; the metric is the time from the start of a lazy resume until the
client has made its second round trip on the same connection and exited.
It covers re-attaching the connection plus the first use of it, and is
compared with `cell.resume_us` and `cell.loopback_rtt_us`.

## Risks / Trade-offs

- Parked connections hold memory while a snapshot waits (at most 256 KiB
  per loopback connection, 128 KiB per card connection); they count
  towards `SOCKETS_MAX`.
- A remote host may give up on a long zero window; that is TCP's
  behaviour for any reader that stops reading.
- A parked listener holds its port while the snapshot waits: another cell
  cannot bind it meanwhile (it could not while the cell ran either).
- The card is polled whenever the kernel's scheduler loop runs (each step,
  and while cells wait on descriptors), as for the connection of a cell
  that sleeps: a host that runs nothing for a long time delays the ACKs of
  parked connections until it runs again; TCP retransmits meanwhile.
