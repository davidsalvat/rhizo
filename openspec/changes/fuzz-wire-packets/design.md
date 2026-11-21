# Design

## D1. A property test, like the other fuzzers

The fuzzer is a `proptest` property in `crates/rhizo-net/src/hostile.rs`,
run by `cargo xtask check` at the fixed seed of `.cargo/config.toml` and
weekly with a random seed and 4096 cases, as the fuzzers of the second
review are. It drives the real `Net::poll_wire` through the existing
`rhizo_hal::testing::QueueLink`, so it covers `smoltcp` as configured in
the kernel (features, buffer sizes, timeouts) and Rhizo's glue together.
`cargo-fuzz` would add coverage guidance but needs a nightly toolchain and
a tool the project does not use; structure-aware mutation of valid frames
(D2) reaches the same parsers without it. No new dependency: `proptest`
is already a dev-dependency of `rhizo-net`.

## D2. The world and its inputs

One `Net` with address 10.0.0.2/24 and two cells' sockets:

- cell A (the victim): a TCP connection to a well-behaved `smoltcp` peer
  at 10.0.0.1 (the echo of `wire_tests`), and a loopback connection;
- cell B (the attacker's): a TCP connection to the attacker's own
  `smoltcp` stack at 10.0.0.66, a connection to a port of the attacker
  that nobody listens on (the test answers its SYN with crafted segments,
  so handshake options such as a zero MSS or a window scale past 14 reach
  a socket in `SYN-SENT`), a UDP socket that sent to the attacker, and a
  listener on port 8080 (listeners are loopback only, so a SYN from the
  wire must not reach it).

A case is up to 40 steps: time passing (1 ms ticks, or seconds in 50 ms
ticks for retransmissions and timeouts), the cells sending, a seed frame,
a frame the attacker's stack sent earlier replayed, random bytes, or a
flood. Seeds are valid frames: SYN to the listener, to a closed port and
to a loopback address, SYN with odd options, segments to B's connection
and to A's port, SYN-ACK and data on the crafted connection, UDP to B's
socket, to a closed port, to the broadcast address and as a jumbo frame,
first and later IPv4 fragments, IPv4 options, ICMP echo and unreachable,
ARP request and reply, a DHCP offer and other Ethernet types. A seed or a
replayed frame then gets up to three bytes changed (mostly in the first 80
bytes, where the headers are), maybe a cut, and in most cases its IPv4,
TCP, UDP or ICMP checksums fixed, so changes get past the checksums to the
parsers and state machines behind them. A test checks that the seeds do
reach their targets (a reset, an ICMP error, an ARP reply, a datagram, a
handshake, data in order).

A second property starts a card on DHCP and plays replies (offer, ACK,
NAK or any type, any offered address and lease) to the client's last
transaction, changed and cut, random frames and time passing.

## D3. What the attacker may not do, and what it may

Frames that claim to come from the peer (its MAC address as source, or its
IP address as the IPv4 or ARP sender) are not injected. TCP and ARP have no
authentication: a host that can send those can reset A's connection with
an in-window RST, or redirect its frames with a forged ARP reply, on any
TCP/IP stack; that is a property of the network, not of the kernel, and
cells needing more use TLS. Everything else an attacker may do to its own
connections (reset them, fill B's UDP queue, stall B's handshake) is
allowed. A rogue DHCP server may change the card's address; the property
there is no panic and an address that is unicast, if any.

## D4. Invariants

After every step: no panic, exactly the eight sockets open, B's listener
has nothing to accept, and every frame the card sends is checked: no frame
holds the loopback connection's bytes, and none but those to the peer's
MAC address holds cell A's bytes. At the end of the case cell A sends once
more and must get back exactly every byte it sent, with no error on its
socket and the same addresses; the loopback bytes are read back intact.
Then every socket is closed and 75 s pass (past the handshake timeout and
the 60 s linger): no socket open, nothing left in `smoltcp`, and the heap
back to its level after a first pass through the same scenario (which gives
every buffer that keeps its room that room).

The heap is measured by a counting `#[global_allocator]` in the test
binary of `rhizo-net` that forwards to `System` and counts bytes per thread
(tests run in parallel); it is test code, its `unsafe` is the
`GlobalAlloc` contract passed through unchanged.

## D5. One ring of frames per poll (S22)

`smoltcp`'s `Interface::poll` handles received frames until the device has
none (its documentation warns that this is unbounded). The virtio-net
driver re-posts each buffer as it hands a frame over, so a host that sends
faster than the kernel handles kept `Net::poll_wire`, and the scheduler
loop on the CPU that polls, busy for the whole flood; cells' own frames
waited too (egress runs after ingress). `Phy`, the device `smoltcp` sees,
now hands over at most `POLL_FRAMES` = 64 frames, the size of the driver's
receive ring (`rhizo_virtio::QUEUE_SIZE`; `rhizo-net` may not depend on
`rhizo-virtio`, so the number is repeated with a reference). When it hands
over all 64, `poll_wire` returns `Some(0)`: the kernel already clamps the
next poll to `WIRE_PERIOD_NS` (20 µs) later, so cells run between polls
and the card is drained at up to 64 frames per 20 µs. A ring holds at most
64 frames, so no benchmark or ordinary load is slowed: more than 64 frames
waiting at one poll means the ring had already refilled during the poll.

## D6. Idle connections stay open (S23)

`smoltcp` 0.14's `set_timeout` is documented to abort a connection when the
peer does not answer the handshake, or is silent while data is
outstanding; its `poll_at` and `dispatch` also abort an idle connection
once nothing has arrived for the timeout. Rhizo set 20 s on every
connection to the outside, so an idle keep-alive connection, or a request
whose answer took more than 20 s after the server acknowledged it, was
reset. The fix keeps the 20 s for the handshake and clears the timeout
when `poll_wire` first sees the connection open (where it sets
`ever_open`). A peer that disappears while data is outstanding is then
retransmitted to with backoff until the cell closes the connection (then
the 60 s linger aborts it), as on Linux until `tcp_retries2`.
`smoltcp` itself is not patched (it is a crates.io dependency); the
behaviour and a reproduction are recorded in `docs/security.md`.
