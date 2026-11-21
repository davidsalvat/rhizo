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

### Requirement: Any OpenAI-compatible model
The agent SHALL take the model's base URL and name as arguments, speak
`/v1/chat/completions` with tools, read answers sent whole or in chunks,
and complete a turn whose answer calls no tool by asking again.

#### Scenario: A model that does not call the tool
- **WHEN** the agent runs as a Linux process against a model that answers every request with text, in chunks
- **THEN** every turn and branch completes with that reply and the agent exits with 0

### Requirement: Live demo with a real model
`cargo xtask demo agent --llm URL --model NAME` SHALL boot the kernel with a
network card on the chosen VMM, under KVM or emulation, and run the agent
against that model for three turns with a suspend and resume between turns
and a fork into three branches, printing each step as it happens with the
model's time apart from the time spent in the cell and by the host.

#### Scenario: Demo against a local model
- **WHEN** a developer runs the demo against a model listening on the host's loopback, on QEMU
- **THEN** the guest reaches it at 10.0.2.2, each turn and branch is printed with its model and Python times, each suspend, resume and the fork with the host's times, and the run ends with `rhizo-demo done`
