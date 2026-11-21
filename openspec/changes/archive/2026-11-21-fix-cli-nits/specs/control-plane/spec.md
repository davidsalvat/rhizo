# Spec Delta

## ADDED Requirements

### Requirement: Sequential cell identifiers
The control kernel SHALL give the cells the host starts or forks
identifiers from 1, each one more than the last given; suspending and
resuming a cell SHALL take none.

#### Scenario: Fork after a resume
- **WHEN** the host suspends cell `N`, resumes it and forks it into two, with no other cell started in between
- **THEN** the children are `N+1` and `N+2`

### Requirement: Kernel build reused
Building a kernel again with nothing changed SHALL compile nothing, whether
or not the optional cells (MicroPython, CPython) are built, so that an
unchanged `rhizo up` boots the kernel it built before.

#### Scenario: Second build
- **WHEN** the `control` test step builds the control kernel again, unchanged
- **THEN** cargo reports the kernel fresh

### Requirement: State after down
After `rhizo down` the state directory SHALL hold only `console.log`, the
last VM's console, and `rhizo up` SHALL first remove what an earlier server
left there but that log.

#### Scenario: Down
- **WHEN** the `control` test step runs `rhizo down`
- **THEN** the state directory holds only `console.log`
