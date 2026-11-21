## MODIFIED Requirements

### Requirement: Tests do not depend on their order
An in-kernel test SHALL pass whether or not the tests before it ran: work
done once per boot (such as bringing up the network card) SHALL happen at
boot, and a test that compares free memory SHALL warm the kernel heap
first. A test that compares memory before and after itself SHALL take
both measures with no connection through the network card still closing,
waiting a bounded time for those an earlier test closed, and SHALL fail
when one never finishes closing.

#### Scenario: Memory check first in its VM
- **WHEN** a test that checks every frame is returned runs first after boot
- **THEN** it passes as it does after the other tests

#### Scenario: A connection still closing from an earlier test
- **WHEN** a test that checks every heap allocation is returned runs right after a test whose connection through the card is still closing
- **THEN** the connection finishes closing before the first measure, and the check counts only the test's own allocations

#### Scenario: Any split of the suite
- **WHEN** `ktest` or `ktest-smp` runs with `RHIZO_SHARDS` from 1 to 4
- **THEN** every VM passes
