# Proposal

## Why

`docs/audit-scope.md` (boundary B5, question Q11) found a gap: frames from
the network reach the kernel through the virtio-net card and are handled
by `smoltcp` (0BSD) and Rhizo's wire code (`crates/rhizo-net/src/wire.rs`),
but `docs/security.md` treats the card as trusted and reviewed the sockets
only against hostile cells. No review or fuzzer has treated what arrives
from the network as hostile input, although anyone on the network the VM
is attached to can send it.

## What Changes

- A review of B5 in `docs/security.md`: a third attacker (a host on the
  network), what it may and may not do, and the findings.
- A host fuzzer in `rhizo-net` (property tests with a fixed seed, as the
  other fuzzers): a `Net` on the `QueueLink` test double with two cells'
  sockets, a well-behaved peer and an attacker sending valid frames (SYN
  to a listening port, data on its own connections, a handshake it answers
  itself, UDP, IPv4 fragments and options, ICMP, ARP, DHCP offers and
  acknowledgements), the same with bytes changed and cut, its own stack's
  frames replayed, random bytes and floods. Invariants: no panic, no
  socket opened or lost, the other cell's connection and loopback sockets
  unaffected, its bytes sent only to its peer, and the kernel heap back to
  where it was once every socket is closed. A second property feeds
  changed, cut and random DHCP replies to a card asking for a lease.
- What it found:
  - S22 (Medium): one poll of the card took frames for as long as they
    kept coming (`smoltcp`'s `Interface::poll` loops until the device is
    empty), so a flood held the polling CPU, and the cells on it, for as
    long as it lasted. A poll now takes at most one ring (64 frames) and
    asks to be polled again at once.
  - S23 (Low, a `smoltcp` 0.14 behaviour): the 20 s timeout set on
    connections to the outside also aborted an open connection on which
    nothing arrived for 20 s, though `smoltcp` documents it for
    outstanding data only. Found at the same time by `fix-tcp-long-wait`,
    which fixed it on `main`; this change records it as a finding only.
- `docs/audit-scope.md` marks B5 as reviewed and fuzzed; the weekly job
  runs the new fuzzer with a random seed.

### Out of scope

Coverage-guided fuzzing (`cargo-fuzz` needs a nightly toolchain and a new
tool), fuzzing the kernel binary or the virtio driver's rings (B6, a
trusted device), and attacks on a connection by a host that can
impersonate its peer (no TCP/IP stack without authentication resists
them: design D3).

## Capabilities

### Modified Capabilities

- `networking`: frames from the network are hostile input; a flood
  cannot hold the kernel.

## Impact

- `crates/rhizo-net` (`wire.rs`: poll budget; `hostile.rs`:
  the fuzzer; `wire_tests.rs`: regression tests), `docs/security.md`,
  `docs/audit-scope.md`, `.github/workflows/weekly.yml`.
- No new dependency. No benchmarked path changes: the benchmarks never
  have more than a ring of frames waiting at one poll. Host tests take about 7 s longer
  (they run in parallel with the others).
