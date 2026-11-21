## MODIFIED Requirements

### Requirement: Split in-kernel tests
`RHIZO_SHARDS=N` SHALL split the in-kernel tests of `ktest` and `ktest-smp`
over `N` VMs that together run each test exactly once. A failing VM's
reproduce command SHALL carry `RHIZO_SHARDS=N`. CI SHALL run the AArch64
in-kernel tests split this way.

#### Scenario: Two shares
- **WHEN** `RHIZO_SHARDS=2 cargo xtask test --only ktest` runs
- **THEN** two VMs run, each prints the tests it ran, and every test runs in one of them

#### Scenario: A share fails
- **WHEN** a test fails in one VM of `RHIZO_SHARDS=2 cargo xtask test --only ktest`
- **THEN** the reproduce command printed starts with `RHIZO_SHARDS=2`
