## MODIFIED Requirements

### Requirement: Node.js in a cell
The official Linux build of a pinned Node.js LTS release, dynamically
linked, SHALL run unmodified as a cell with its JIT (no `--jitless`),
started from the cell's files with its interpreter and libraries, and
print what the same files print on Linux with an empty environment, HTTP
requests through the network card with `node:http` included.

#### Scenario: Agent request
- **WHEN** Node.js runs `/app/demo.js` in a cell holding the packed files
- **THEN** it prints the output recorded from Linux, nothing else, and exits with 0

#### Scenario: A request to the host
- **WHEN** it runs with the URL of the host's fake model server
- **THEN** it also prints the tool call the model answered, as on Linux

#### Scenario: Same behavior on Linux
- **WHEN** the differential check runs the same files natively on a Linux host of the same architecture
- **THEN** they print the recorded output and nothing on standard error

## ADDED Requirements

### Requirement: Code written at run time
A statically linked C program that writes machine code into memory it maps
writable and executable, runs it, rewrites it and runs it again, also from
two threads at once, SHALL print in a cell what it prints on Linux, also
when forked, suspended and resumed between its steps.

#### Scenario: A small JIT
- **WHEN** `libc_jit` runs in a cell, plain, forked at `ready`, or suspended and resumed there
- **THEN** every copy prints its expected output and exits with 0
