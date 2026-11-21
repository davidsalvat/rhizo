## MODIFIED Requirements

### Requirement: A share of the in-kernel tests
An in-kernel test build booted with `shard=I/N` (`0 <= I < N`) SHALL run
only the tests dealt to share `I`, reporting how many it runs, and SHALL
ignore a malformed value. The tests SHALL be ranked by their estimated
cost, most expensive first (1 unless given, ties by position across all
test suites), and dealt by rank to shares `0` to `N-1`, then `N-1` to
`0`, and again; each share SHALL run its tests in suite order. The deal
SHALL depend only on the suites and `N`.

#### Scenario: Shares cover every test once
- **WHEN** the test build boots `N` times with `shard=0/N` to `shard=N-1/N`
- **THEN** every test runs in exactly one of the boots

#### Scenario: Expensive tests apart
- **WHEN** the two most expensive tests sit at positions of the same parity and the build boots with `shard=0/2` and `shard=1/2`
- **THEN** they run in different boots, each boot in suite order

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
