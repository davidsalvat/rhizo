# Proposal

## Why

On Firecracker and Cloud Hypervisor, a laptop turn (`workload.agent_turn_us`)
takes exactly 1 s in most benchmark runs, against 0.25 to 0.35 ms on QEMU.
The reference workloads change thought the guest stopped polling its card.
A diagnostic run (phases of the slowest turn, and the card's polls and
frames, in draft #71) shows that it does not:

- the guest polled the card 418,000 times during the turns;
- the whole second is spent in `connect`, so the first SYN goes unanswered
  and smoltcp sends it again after its 1 s retransmission timeout;
- the slow runs sent 21 more frames and received 10 more;
- the first run on each VMM is always fast, and the second and third are
  slow.

The cause is the ephemeral ports. Every boot starts them at 32768, the
guest's address on the tap is fixed, and the fake model server keeps its
port across the runs of one benchmark. The first connections of a new
boot therefore have the same addresses and ports as connections of the
previous run, which the host's Linux still keeps in `TIME_WAIT` (it closed
them first). Linux takes a SYN for a connection in `TIME_WAIT` only if the
new initial sequence number is above the old one. smoltcp picks it at
random, so in about half the cases the host answers with an ACK for the
old connection. smoltcp resets it and sends the SYN again 1 s later. QEMU's
user network is a new process for every run, with no `TIME_WAIT` left
over. A real cell host that restarts behind a fixed address would hit the
same stall with every server it talks to.

## What Changes

- **Ephemeral ports start at a place each boot chooses** (RFC 6056): the
  boot identity the kernel already gives the sockets (its randomness) picks
  the first port, then ports go on in order as before. Two boots almost
  never reuse each other's first connections.
- **Gate the agent turns**: `workload.agent_turn_us` and
  `laptop.resume_turn_us` were ungated as bimodal. Once two CI runs agree,
  they are gated.

### Out of scope

Interrupts from the network card (the guest does poll, every 20 µs while a
cell uses the card: that costs CPU while an agent waits for a slow model,
but no latency; separate change `add-virtio-interrupts`). Initial sequence
numbers that grow with time (RFC 6528): smoltcp picks them, and time
restarts at every boot.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `networking`: ephemeral ports per boot.

## Impact

`crates/rhizo-net/src/lib.rs` (`set_boot`), its tests,
`bench/README.md`, `bench/baseline.json`.
