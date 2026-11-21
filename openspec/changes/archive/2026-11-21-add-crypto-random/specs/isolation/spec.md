# Spec Delta

## ADDED Requirements

### Requirement: Random bytes are unpredictable
The bytes the kernel gives a cell (`getrandom` and `AT_RANDOM`) SHALL come
from a ChaCha20 generator with fast key erasure, seeded from the CPU's
hardware random source, which enters the kernel only through
`rhizo_hal::Entropy`. When the CPU has no hardware source the kernel SHALL
say at start that `getrandom` is predictable.

#### Scenario: Generator checked against the standard
- **WHEN** the ChaCha20 block function is run on the test vectors of RFC 8439
- **THEN** it produces the RFC's key stream

#### Scenario: Earlier output stays secret
- **WHEN** the state of a generator is read after it answered a request
- **THEN** that state does not produce the bytes already answered

### Requirement: Random streams are private to each cell
Each cell SHALL draw from its own generator. A fork, a copy of a template
and a resumed snapshot SHALL NOT draw what any other cell draws: no
generator state is copied into a child or kept in a snapshot.

#### Scenario: Copies of one cell
- **WHEN** a cell that drew random bytes is forked twice, frozen into a template copied twice, and suspended into a snapshot resumed twice (eagerly and lazily), and each copy then calls `getrandom`
- **THEN** every copy, and every independent cell, draws different bytes
