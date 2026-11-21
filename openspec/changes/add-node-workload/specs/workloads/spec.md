# Spec Delta

## ADDED Requirements

### Requirement: Node.js in a cell
The official Linux build of a pinned Node.js LTS release, dynamically
linked, SHALL run unmodified as a cell with `--jitless`, started from the
cell's files with its interpreter and libraries, and print what the same
files print on Linux with an empty environment, HTTP requests through the
network card with `node:http` included.

#### Scenario: Agent request
- **WHEN** Node.js runs `/app/demo.js` with `--jitless` in a cell holding the packed files
- **THEN** it prints the output recorded from Linux, nothing else, and exits with 0

#### Scenario: A request to the host
- **WHEN** it runs with the URL of the host's fake model server
- **THEN** it also prints the tool call the model answered, as on Linux

#### Scenario: Same behavior on Linux
- **WHEN** the differential check runs the same files natively on a Linux host of the same architecture
- **THEN** they print the recorded output and nothing on standard error

### Requirement: Node.js only where downloaded
Where Node.js is not downloaded (`RHIZO_NODE` unset), the Node.js checks
SHALL report that they did not run, and pass.

#### Scenario: Not downloaded
- **WHEN** `RHIZO_NODE` is not set
- **THEN** the Node.js test step and the differential check say Node.js was not packed, and pass

### Requirement: Warm Node.js templates
A Node.js cell suspended after its warm-up SHALL resume, fork and serve as
a template, every copy answering without warming up again.

#### Scenario: Warm Node.js template
- **WHEN** cells are forked from Node.js frozen after it printed `warm`, or resumed from its snapshot taken there and forked
- **THEN** each prints only the answer and exits with 0
