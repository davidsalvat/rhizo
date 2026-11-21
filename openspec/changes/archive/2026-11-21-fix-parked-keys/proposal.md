# Proposal

## Why

The second adversarial review (D.2 in `docs/security.md`, finding S15)
treats snapshots as untrusted input: templates will be downloaded
(`docs/vision.md`). A parked listener or connection is found again by the
key in its snapshot record (design D2 of `keep-tcp-across-suspend`), and
that key counted up from 1 in each boot, next to a boot identity read from
the cycle counter. A forged snapshot resumed in the same boot could name
the keys of other suspended cells' connections (1, 2, 3, ...) with one
socket record each and take them: read the bytes their peers send, write
in their name, and leave the real owner a reset connection on resume. That
design rated it acceptable because the host was the only source of
snapshot bytes; it is not once snapshots come from outside.

## What Changes

- A parked record's key is SipHash-2-4 of the counter under a 128-bit
  secret the kernel gives `Net` at boot (`Net::set_secret`), never in any
  snapshot. Keys of one boot say nothing about each other or the secret,
  and no two parked sockets share one.
- The format, the boot identity, and what a resume does with a key it
  does not hold (the record's own state: reset, or bound again) do not
  change.

### Out of scope

The secret is as strong as the kernel's randomness: today the cycle
counter (open finding O4); with `add-crypto-random`, its generator. A
forged record still costs the forger nothing but a reset socket.

## Capabilities

### Modified Capabilities

- `networking`: a parked connection comes back only for the record that
  parked it.

## Impact

- `crates/rhizo-net` (keys), `kernel/src/cell.rs` (the secret at boot).
- One hash per socket parked by a suspend; nothing else moves.
