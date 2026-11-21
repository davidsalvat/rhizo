## ADDED Requirements

### Requirement: A share of the in-kernel tests
An in-kernel test build booted with `shard=I/N` (`0 <= I < N`) SHALL run
only the tests whose position across all test suites is `I` modulo `N`,
reporting how many it runs, and SHALL ignore a malformed value.

#### Scenario: Shares cover every test once
- **WHEN** the test build boots `N` times with `shard=0/N` to `shard=N-1/N`
- **THEN** every test runs in exactly one of the boots

### Requirement: Tests do not depend on their order
An in-kernel test SHALL pass whether or not the tests before it ran: work
done once per boot (such as bringing up the network card) SHALL happen at
boot, and a test that compares free memory SHALL warm the kernel heap
first.

#### Scenario: Memory check first in its VM
- **WHEN** a test that checks every frame is returned runs first after boot
- **THEN** it passes as it does after the other tests
