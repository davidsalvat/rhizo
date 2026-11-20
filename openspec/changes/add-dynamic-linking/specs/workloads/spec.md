# Spec Delta

## ADDED Requirements

### Requirement: Distribution programs in cells
Dynamically linked programs SHALL run as cells unmodified, with the
distribution's interpreter and libraries in the cell's files, and print
what they print on Linux run the same way (from the top of the same files,
with an empty environment): a C program linked dynamically with glibc,
and, where the cells' architecture is the host's, the host's `ls`, `sort`
and `python3`. Programs not packed SHALL be reported as not run.

#### Scenario: Coreutils
- **WHEN** `ls data` and `sort data/fruit` run as cells holding the distribution's `ls`, `sort`, `ld.so` and their libraries
- **THEN** each prints the output recorded from Linux and exits with 0

#### Scenario: A dynamically linked interpreter
- **WHEN** the distribution's `python3 -S app/demo.py` runs as a cell, its script importing `json`, whose C accelerator it loads with `dlopen`
- **THEN** it prints the output recorded from Linux and exits with 0

#### Scenario: Same behavior on Linux
- **WHEN** the differential check runs each packed program natively from the top of the packed tree with an empty environment
- **THEN** each prints the recorded output and exits with 0
