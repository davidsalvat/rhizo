# Design

## Root cause

`Net::wire_connect` calls `tcp::Socket::set_timeout(Some(20 s))`. In
`smoltcp` 0.14, `dispatch` aborts the socket (state `Closed`, RST sent)
when `now >= remote_last_ts + timeout`, and `poll_at` schedules a poll for
that moment, in every state. `remote_last_ts` is the time of the peer's
last segment; it is cleared only when the send buffer goes from empty to
non-empty (`send_impl`), so that the count then starts at the first
transmission. Nothing clears it once everything sent has been
acknowledged. The documented contract ("there is data in the transmit
buffer") is therefore not what the code does.

The sequence on a chat turn, measured by the host test
`a_connection_silent_for_long_still_gets_its_reply` before the fix (frames
on the in-memory wire, times from the send):

| t (ms) | frame |
|---|---|
| 1 | card → peer, PSH\|ACK, the request |
| 11 | peer → card, ACK (`remote_last_ts`) |
| 20,011 | card → peer, RST\|ACK; the socket reports `ECONNRESET` |

and on QEMU `microvm` with the CPython chat agent against a fake model
that answers after 45 s (server's clock): request 1 at 6.288 s; reset 20 s
later; new connection and the same request at 26.383 s, in the same
millisecond (the reconnection is prompt); reset 20 s later; the agent
gives up 40.3 s after the user's line.

Ruled out by the same measurements: ARP (no frame is lost or delayed:
the reconnect's SYN is answered at once), ephemeral ports (each
connection takes the next port), the wake path (the reset is reported in
the same poll that sends it), retransmission backoff (nothing is
retransmitted while the request is acknowledged).

## D1. The timeout runs only while something is outstanding

A wire connection carries `smoltcp`'s timeout while it connects and while
its send buffer holds bytes (sent and not yet acknowledged, or not yet
sent), and none otherwise:

- `wire_connect` sets it (as before);
- `wire_send` sets it before queuing bytes. `smoltcp` restarts the count
  from the first transmission when the buffer was empty, so the time spent
  idle before does not count;
- `poll_wire`, when a frame changed the sockets (it already visits each
  wire socket then, to wake its waiters and mark opened connections),
  clears it on each open connection whose send buffer is empty. An
  acknowledgment that empties the buffer arrives in a frame, so it always
  passes there; the abort check of that same `smoltcp` poll sees a fresh
  `remote_last_ts`.

A peer that disappears while bytes wait is still given up after 20 s, as
before. A peer that is silent with nothing owed to it, for 45 s or for an
hour, is waited for, as a Linux socket without `SO_KEEPALIVE` waits.

Lost:

- `smoltcp` keep-alive (probe every 10 s, timeout 20 s): one line, but it
  sends probes on every idle connection and aborts idle connections
  whose path hiccups for 20 s, which Linux does not do by default.
- Dropping the timeout altogether: a SYN or bytes to a vanished host would
  be retransmitted forever and a blocked `connect` or `send` never return.
- Patching `smoltcp`: a vendored fork for a two-line policy we can apply
  from outside.

## D2. Closed connections: the linger time alone

`wire_close` clears the timeout: a connection the cells closed is given up
by `LINGER` (60 s, Linux's `FIN_WAIT_2` time), as its documentation says.
Before, a closed connection still carrying the timeout was aborted 20 s
after the peer's last segment, so `LINGER` only applied to connections
that had last been idle.

## D3. Testing the timeout quickly

The timeout is 20 s; an in-kernel test that outlasts it would add 20 s to
the in-kernel step. `Net::set_wire_timeout` sets the timeout given to
connections opened or written to from then on; the in-kernel test sets
500 ms, keeps a connection to the host's echo server silent for 1.5 s,
checks an echo, closes it, and checks that a new connection is open and
echoes within a second (the new `quiet` cell), then restores 20 s. It
is a program of its own rather than a mode of `loopback`: a first version
added to `loopback` grew the code `loopback twice` touches, and the lazy
resume behind `cell.tcp_resume_insns` cost 1,903 more instructions on
every run (+2.6 %, over the gate); as a separate program the count is
main's again. Without D1 that test fails on its second send (`EPIPE`,
the connection was reset). The host tests of `rhizo-net` run on simulated
time and use the real 20 s.

## D4. Cost

One store per `send` on a wire connection, and one check of the send
queue per wire socket when a frame changes the sockets (the loop already
visits them). Measured under `-icount` on QEMU `microvm` x86_64,
`cargo xtask --arch x86_64 bench --vmm qemu-microvm --icount-only`, against
`main` at the merge base: 48 of the 50 instruction counts identical
(`cell.loopback_rtt_insns` 8,950, `cell.tcp_resume_insns` 74,731);
`breakdown.fork_touch.frames_insns` and `breakdown.fork_touch.total_insns`
+13 (+0.008 % and +0.0002 %). Timed medians of 5 runs under TCG, before and after (an earlier
base): `net.tcp_rtt_us` 127.5 and 116.6, `cell.loopback_rtt_us` 31.7 and
34.5, `cell.tcp_resume_us` 353 and 400, within TCG's noise.

## Manual reproduction

The realistic reproduction stays manual (it takes minutes): `rhizo up
--arch x86_64`, a fake model answering after 45 s, `rhizo chat` with two
turns 60 s apart; and the same model answering after 2 s and closing the
connection after each answer (a server's keep-alive timeout), where each
next turn reconnects.
