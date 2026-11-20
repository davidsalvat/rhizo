# Spec Delta

## ADDED Requirements

### Requirement: CPython in a cell
A static build of a pinned CPython release SHALL run as a cell, unmodified,
with the standard-library modules it uses in the cell's files, and print
what the same executable prints on Linux with the same files and an empty
environment; cells forked from a template frozen after its warm-up SHALL
answer without warming up again. Where CPython's source is not given, the
checks SHALL report that they did not run.

#### Scenario: Agent request
- **WHEN** CPython runs `/app/agent_demo.py` in a cell holding the packed standard library under `/lib/python3.13`
- **THEN** it prints the output recorded from Linux, nothing else, and exits with 0

#### Scenario: Warm CPython template
- **WHEN** cells are forked from CPython frozen after it printed `warm`
- **THEN** each prints only the answer and exits with 0

#### Scenario: Same behavior on Linux
- **WHEN** the differential check runs the same CPython on the same files on a Linux host of the same architecture
- **THEN** it prints the recorded output and nothing on standard error

#### Scenario: Not built
- **WHEN** `RHIZO_CPYTHON_SRC` is not set
- **THEN** the in-kernel test and the differential check say CPython was not built, and pass
