# Spec Delta

## MODIFIED Requirements

### Requirement: Hung test runs are terminated
The in-kernel test command SHALL stop a test VM that prints nothing for a
configured time limit and report the run as failed; a run that keeps
printing progress is not stopped however long it takes.

#### Scenario: Kernel hangs during tests
- **WHEN** a test VM produces no output and no exit status for the time limit
- **THEN** the VM is terminated
- **AND** the command reports a timeout and exits with a non-zero status

#### Scenario: Long suite
- **WHEN** a test VM keeps printing test results for longer than the time limit in total
- **THEN** it is not stopped
