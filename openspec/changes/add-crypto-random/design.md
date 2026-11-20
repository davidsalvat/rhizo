# Design

## D1. ChaCha20 with fast key erasure, written here

The generator is the ChaCha20 block function (RFC 8439) keyed with 32
bytes. Each request computes block 0 under the current key: its first 32
bytes become the next key, its last 32 the first output bytes; blocks 1,
2, ... give the rest. The old key is overwritten before the call returns,
so reading the kernel's memory later does not reveal bytes already handed
out (backtracking resistance), and nothing but the key is kept.

About seventy lines, tested on the host against the RFC's vectors
(section 2.3.2 and appendix A.1), no `unsafe`, no dependency.

Alternative: the `rand_chacha` crate (MIT/Apache-2.0). Rejected: it brings
`rand_core` and `ppv-lite86` for one function the kernel can carry, and
its SIMD paths do not apply (the kernel is built without SSE/NEON).

Alternative: a hash-based DRBG (SHA-256). Rejected: slower without hardware
help and longer to write; ChaCha20 is what Linux and the BSDs use for the
same job.

## D2. One kernel generator, one per cell split off on first use

`cell::init` seeds the kernel's generator from `rhizo_hal::Entropy`
(arch `random::HardwareEntropy`). A cell holds `Option<Generator>`: `None`
until its first `getrandom`, which keys the cell's generator with 32 bytes
of the kernel's. `AT_RANDOM` (16 bytes on the initial stack) comes straight
from the kernel's generator at spawn.

- **Forks and templates.** A child starts with `None`, never a copy of the
  parent's generator. `Generator` is not `Clone`, so the compiler refuses a
  copy.
- **Snapshots.** The generator is not part of a cell's state: a snapshot
  holds none, and every resume starts with `None`. Two resumes of one
  snapshot therefore draw different bytes, and neither replays what the
  suspended cell would have drawn. The snapshot format does not change.
- **Cost.** Splitting is one ChaCha20 block, paid only by cells that ask.
  Forks (`cell.fork_us`) and resumes do nothing for randomness.

Alternative: one shared generator for every call. Rejected: every
`getrandom` would take a lock shared by all CPUs; per-cell generators lock
only to be created.

Alternative: store the generator in snapshots and reseed on resume.
Rejected: keeping secret state in a file the host moves around is a leak,
and reseeding needs the same split anyway.

## D3. Hardware entropy through `rhizo-hal`

`rhizo_hal::Entropy::fill(&mut [u8]) -> bool` is the only way randomness
enters the kernel. On x86_64 it uses `RDSEED` (the conditioned entropy
source, up to 100 tries per word), falling back to `RDRAND` (ten tries, as
Intel's guide advises); on AArch64 `RNDR` (ten tries). The presence of each
is read from CPUID / `ID_AA64ISAR0_EL1`. The kernel's generator is seeded
once, at start: the hardware source is only needed for the 32 bytes of
the seed.

Without a source (`fill` returns `false`), the seed is the cycle counter
and the kernel prints `rhizo: no hardware random source: getrandom is
predictable`. The tests on such a CPU say so instead of failing.

Alternative: virtio-rng. Deferred: every platform in CI has the
instruction, and a device driver is more code than this change; it is the
way to cover AArch64 hosts without FEAT_RNG.
