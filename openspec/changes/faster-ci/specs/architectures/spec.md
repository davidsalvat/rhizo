## MODIFIED Requirements

### Requirement: Both architectures pass every spec
Every requirement of the kernel specs SHALL hold on both architectures, and CI
SHALL run the full test suite for each on every pull request that changes
code and on every push to main.

#### Scenario: Pull request
- **WHEN** a pull request that changes code is opened
- **THEN** CI runs `cargo xtask ci` for AArch64 and for x86_64, and both must pass
