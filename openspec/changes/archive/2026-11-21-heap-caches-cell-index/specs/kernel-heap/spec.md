## ADDED Requirements

### Requirement: Small blocks waste less than a power of two
The kernel heap SHALL serve a small request aligned to 16 bytes or less
from the smallest of its size classes that holds it, with classes between
the powers of two (48 to 1360 bytes, each the largest multiple of 16 bytes
of which a page holds a whole number) as well as the powers of two from 16
to 2048 bytes. A request aligned to more than 16 bytes SHALL be served from
the smallest power-of-two class that holds both its size and its
alignment.

#### Scenario: A third of a page
- **WHEN** three requests of 1350 bytes aligned to 8 bytes are made on an empty heap
- **THEN** they share one page, and their bytes read back what was written

#### Scenario: Aligned beyond 16 bytes
- **WHEN** a request of 48 bytes aligned to 32 bytes is made
- **THEN** it is served from the 64-byte class, at an address that is a multiple of 32
