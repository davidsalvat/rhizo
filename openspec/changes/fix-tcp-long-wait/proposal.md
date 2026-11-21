# Proposal

## Why

On a KVM host, `rhizo chat` against a local model (Ollama, llama-server)
fails whenever the model takes longer than about 20 s to answer:

- the agent's socket gets `ConnectionResetError: Connection reset by
  peer`, although the server did answer;
- `ask()` in `cells/python/agent_chat.py` then opens a new connection and
  sends the request again, and that attempt fails the same way about 20 s
  later, so the turn ends in `[agent] the model failed` after about 40 s.

Reproduced on QEMU `microvm` (x86_64, TCG) with a fake model that answers
after 45 s. The model server logs the request at 6.288 s, a second
connection and request at 26.383 s (the retry, opened in under 1 ms), and
the agent reports the reset at 40.3 s; both answers, written at 51.3 s and
71.4 s, go to connections the guest has already dropped. A host test of
`rhizo-net` with a `smoltcp` peer that holds its echo for 45 s shows the
cause: the request is acknowledged 10 ms after it is sent, nothing crosses
the wire after that, and exactly 20,000 ms after the peer's acknowledgment
the card sends RST|ACK and the socket reports `ECONNRESET`.

The card sets `smoltcp`'s timeout (20 s) on every connection. `smoltcp`
documents that timeout as applying while connecting or while sent data
waits, but its implementation counts it from the peer's last segment
whatever the state: an established connection with nothing to send is
aborted after 20 s of silence. A model that thinks for 20 s, or an idle
kept-alive connection between two turns, is therefore reset by Rhizo
itself. The slow reconnect is the same bug: the retried request is
aborted after the same 20 s; the reconnection itself is prompt.

## What Changes

- **Silence is not a failure.** The card's 20 s timeout applies only while
  a connection is being opened and while bytes it sent wait for their
  acknowledgment. An open connection with nothing outstanding waits for
  its peer as long as it takes, as a Linux socket without `SO_KEEPALIVE`
  does.
- **Closed connections end by the linger time alone** (60 s), as their
  documentation already says.
- **Tests**: host tests of a 45 s silent answer, of a 10-minute idle
  connection, of bytes a vanished peer never acknowledges (still given up
  at 20 s), and of reconnecting after a reset or after the peer ends a
  kept-alive connection; an in-kernel test through the real card with the
  timeout shortened to 500 ms (silent for 1.5 s, then a new connection
  open and echoing within a second).

### Out of scope

`SO_KEEPALIVE` and the `TCP_KEEP*` options on wire connections; giving up
on unacknowledged bytes after Linux's ~15 minutes instead of 20 s.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `networking`: connections to the outside survive silent peers.

## Impact

`crates/rhizo-net/src/wire.rs` and its tests, `kernel/src/cell.rs`,
`kernel/src/tests.rs`, `cells/src/bin/loopback.rs` (`pause` mode). No
benchmarked path changes beyond one store per send and one check per
wire socket when a frame arrives.
