## ADDED Requirements

### Requirement: AArch64 instruction gate in CI
Every pull request SHALL count the AArch64 kernel's benchmarks in retired
instructions under QEMU with `-icount` (`cargo xtask bench --icount-only`)
on a GitHub-hosted ARM64 runner, and SHALL fail when a gated count is
worse than its baseline in the `ci-arm` environment by more than the
instructions threshold. The results SHALL be published under annotation
titles distinct from the x86_64 job's.

#### Scenario: AArch64-only regression
- **WHEN** a pull request adds more than 2 % of instructions to a gated operation on AArch64 only
- **THEN** the AArch64 CI job fails naming the `qemu-virt-icount` metric

#### Scenario: Same operations on both architectures
- **WHEN** the AArch64 kernel runs under `-icount` with performance counters in the guest
- **THEN** the results hold the fork benchmarks too, counted with the clock where no counter pass covers them

### Requirement: Both architectures in the README
`cargo xtask bench --readme` SHALL accept several results files and merge
them, refusing a metric present in more than one. When AArch64 instruction
counts are present, the README section SHALL show them next to x86_64's;
without them the section SHALL be unchanged.

#### Scenario: Merged results
- **WHEN** `cargo xtask bench --from x86_64.json --from aarch64.json --readme` runs
- **THEN** the README section holds a table of instructions per operation with an x86_64 and an AArch64 column
