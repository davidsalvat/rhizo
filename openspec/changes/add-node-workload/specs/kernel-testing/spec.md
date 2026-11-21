# Spec Delta

## ADDED Requirements

### Requirement: Node.js tests in their own VM
Where Node.js is packed (`RHIZO_NODE`), `cargo xtask test` SHALL run the
Node.js in-kernel tests in a kernel of their own, the only one embedding
Node.js, on a VM with 1 GiB and 2 vCPUs (step `ktest-node`), and SHALL
fail unless the kernel reports `cpus: 2 online` and every test passes.
Without `RHIZO_NODE` the step SHALL say why it did not run. CI SHALL run
it on both architectures.

#### Scenario: Rerun the Node.js tests
- **WHEN** a developer runs `cargo xtask test --only ktest-node` with `RHIZO_NODE` set
- **THEN** the Node.js tests run on 2 vCPUs with 1 GiB and the step reports their result

#### Scenario: Not downloaded
- **WHEN** a developer runs `cargo xtask test` without `RHIZO_NODE`
- **THEN** the step prints that the Node.js tests did not run and why, and the run passes
