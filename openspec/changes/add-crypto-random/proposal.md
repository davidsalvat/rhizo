# Proposal

## Why

`getrandom` and `AT_RANDOM` come from xorshift seeded by the cycle counter
(open item O4 of `docs/security.md`): anyone who knows roughly when a cell
started can predict every byte it is given. Cells will run agent code
that makes keys, tokens and TLS sessions, and glibc seeds its stack
protector and pointer guard from `AT_RANDOM`. Forks and templates make it
worse in principle: copies of one cell must never be handed the same
bytes, and a snapshot resumed twice must not replay what the first resume
drew.

## What Changes

- A narrow `rhizo_hal::Entropy` interface for the CPU's random-number
  instruction: `RDSEED`, else `RDRAND`, on x86_64; `RNDR` (FEAT_RNG) on
  AArch64. A test double makes the logic deterministic on the host.
- A small ChaCha20 (RFC 8439) generator with fast key erasure in
  `rhizo-core` (`random`), checked against the RFC's test vectors: no new
  dependency.
- The kernel seeds one generator at start from the hardware. Each cell
  splits its own generator off it the first time it asks for random
  bytes; `AT_RANDOM` and the network stack's seed come from it too.
- A cell's generator is never copied: a fork, a copy of a template and a
  resumed snapshot start without one and split a new one off the kernel's
  when they first ask, so they never share output with each other or with
  the cell they came from. Snapshots hold no generator state (the format
  is unchanged).
- Without a hardware source the kernel says so at start (`getrandom is
  predictable`) and falls back to the cycle counter.

### Out of scope

A virtio-rng driver (every platform Rhizo runs on today has the
instruction: QEMU's `max` CPU on both architectures and the x86_64 hosts
under Firecracker and Cloud Hypervisor); reseeding from the hardware while
running; randomness kept in a cell's own memory (a libc's buffered state,
the stack protector value) is memory like any other and is copied by a
fork or a snapshot.

## Capabilities

### Modified Capabilities

- `isolation`: random bytes given to cells are unpredictable and private
  to each cell, fork and resume.

## Impact

`crates/rhizo-hal` (`Entropy`, `FakeEntropy`), `crates/rhizo-core`
(`random`), `kernel/src/arch/*/random.rs`, `kernel/src/cell.rs`,
`kernel/src/tests.rs`, `cells/src/bin/dice.rs`, `docs/security.md`
(O4 closed), `AGENTS.md` (arch interface). Spawning computes one ChaCha20
block instead of 16 xorshift steps for `AT_RANDOM`; forks and resumes do
no work for randomness until a cell asks.
