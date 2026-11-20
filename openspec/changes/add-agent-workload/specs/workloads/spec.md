# Spec Delta

## ADDED Requirements

### Requirement: A real agent end to end
A CPython agent SHALL run as a cell, turn after turn talking over one
kept-alive HTTP connection to the host's fake model and running the tool it
asks for; the host SHALL suspend and resume it between turns without
resetting its connection, and fork it into branches at its decision point.
Its transcript SHALL equal the one it prints as a Linux process.

#### Scenario: Turns, suspends and branches
- **WHEN** the agent runs 3 turns, suspended and lazily resumed after each, then forks into 3 branches that each run one more turn
- **THEN** its transcript equals the recorded one, the model's request count on its connection goes on across every suspend, and every branch exits with 0

#### Scenario: Nothing stays open
- **WHEN** the host that ran the agent is dropped
- **THEN** the kernel holds no socket

#### Scenario: Same transcript on Linux
- **WHEN** the differential check runs the same CPython and agent natively in `linux` mode against the same fake model
- **THEN** it prints the recorded transcript and nothing on standard error

#### Scenario: Not available
- **WHEN** CPython is not built or there is no network card and model server
- **THEN** the in-kernel test says it did not run, and passes

### Requirement: The host's safe points in a cell
The host SHALL be able to tell whether a live cell sleeps (none of its
threads can run, and one waits for a deadline, none for a descriptor), and
to create or replace a file in a live cell's own files.

#### Scenario: Branch number
- **WHEN** the host writes `/branch` into a cell started from a template and the cell reads it
- **THEN** the cell reads what the host wrote, and the template and the other cells started from it do not see it
