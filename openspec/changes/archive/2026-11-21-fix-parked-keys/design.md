# Design

## D1. A keyed function of the counter

Each parked socket gets `siphash24(secret, n)` for the next `n`, skipping a
value another parked socket holds. Alternatives:

| Option | Unguessable | Needs | Cost |
|---|---|---|---|
| **PRF of a counter under a boot secret (chosen)** | yes, without the secret | 16 random bytes at boot | one hash per park |
| A random key per park | yes | randomness in every suspend, passed through `export` | a draw per park |
| Refuse parked records in bytes from outside (`from_bytes`) | yes | a provenance flag on `Snapshot` | a host keeping its own snapshots as bytes loses its connections |
| Keep the counter (as before) | no | — | — |

SipHash-2-4 (Aumasson and Bernstein, 2012) is a pseudorandom function on
short inputs, written in 30 lines against the reference test vector; no
dependency. `rhizo-net` still reads no randomness: the kernel passes the
secret, as it passes the boot identity.
