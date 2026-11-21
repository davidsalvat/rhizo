# Spec Delta

## MODIFIED Requirements

### Requirement: Control kernel
A `control` kernel build SHALL boot, say hello on its control channel, and
then serve the host's requests until asked to power off: run, list,
suspend, resume, fork, freeze and kill cells, stream a cell's output, write
a file into a running cell, save a suspended cell or a template as bytes
and load bytes as one, report memory and cells. It SHALL decide nothing by
itself, and every reply SHALL carry the time the kernel took, in
nanoseconds.

#### Scenario: Power off
- **WHEN** the host asks the kernel to power off
- **THEN** the kernel replies, and the VMM ends

### Requirement: The rhizo tool
`rhizo` SHALL boot a VM with the control kernel (`up`, choosing the VMM,
architecture, memory, CPUs and acceleration), keep a state that later
commands use to reach it, and offer `run` (templates: `python-agent`,
`python`, any program the kernel holds), `ps`, `stats`, `fork`, `suspend`,
`resume`, `freeze`, `save`, `load`, `attach`, `kill` and `down`, each
printing the kernel's time and the round trip the host measured. Several
commands SHALL be able to talk to the same VM at once.

#### Scenario: Argument errors
- **WHEN** a command is unknown, misses its cell, or `run python-agent` misses `--llm` or `--model`
- **THEN** `rhizo` fails with a message and talks to no VM

#### Scenario: Up and down
- **WHEN** `rhizo up` then `rhizo down` run
- **THEN** the first returns once the kernel answers, and the second once the VM and its state are gone

### Requirement: Bounded input requests
The protocol's version SHALL be 3. An `input` request carrying more than
64 KiB, or a `load-data` request carrying more than 64 KiB, SHALL be
refused as too large, and its decoding, like that of every request, SHALL
never panic nor allocate more than the frame's bytes.

#### Scenario: Oversized or malformed input
- **WHEN** the kernel receives an `input` request of 64 KiB plus one byte, or one whose length exceeds its payload
- **THEN** it refuses it and keeps serving

#### Scenario: Oversized snapshot chunk
- **WHEN** a `load-data` request carries 64 KiB plus one byte, or a `load` request a template flag other than 0 or 1
- **THEN** decoding refuses it

### Requirement: Sequential cell identifiers
The control kernel SHALL give the cells the host starts, forks or loads
identifiers from 1, each one more than the last given; suspending,
resuming and freezing a cell SHALL take none.

#### Scenario: Fork after a resume
- **WHEN** the host suspends cell `N`, resumes it and forks it into two, with no other cell started in between
- **THEN** the children are `N+1` and `N+2`

## ADDED Requirements

### Requirement: Templates under control
`freeze` SHALL make a running cell a template: it never runs again, `list`
shows it as a template, `fork` of it SHALL make cells from it (all or
none, each keeping no time of the template's waits), and `kill` SHALL drop
it; its memory goes back once no cell made from it shares it.

#### Scenario: Freeze and fork
- **WHEN** the host freezes the chat agent waiting for its input and later forks the template into two
- **THEN** `rhizo ps` lists it as a template, and each new cell answers its own input

### Requirement: Save
`save` SHALL send the exported snapshot of a suspended cell or a template
in chunks of at most 64 KiB, then their total, leaving the cell as it was.
`rhizo save <id> <file>` SHALL write the file once all its bytes came, and
print its size, the kernel's time, the round trip and the rate.

#### Scenario: Save a template
- **WHEN** the `control` test step saves a template
- **THEN** `rhizo save` reports the file, its size and its rate, and refuses a cell that does not exist

### Requirement: Load
`load` SHALL announce a snapshot's size and whether it becomes a template,
and get the cell it will be, refused when memory cannot hold it.
`load-data` requests SHALL carry its bytes in order; the one completing it
SHALL validate it and make it a suspended cell or a template, or refuse it
with the reason and keep nothing; bytes past the size SHALL refuse it.
`rhizo load <file> [--template]` SHALL send a file and print as `save`.

#### Scenario: A template and a suspended cell across a reboot
- **WHEN** the `control` test step freezes a chat agent into a template after one turn, leaves another suspended after one turn, saves both, runs `rhizo down` and `rhizo up`, loads the first as a template and forks it into two, and loads the second and talks to it
- **THEN** each of the two copies and the resumed cell answers its next message right

#### Scenario: Refused files
- **WHEN** `rhizo load` is given a saved file with one byte changed, its first half only, or the architecture of the other in its header
- **THEN** it fails, saying `checksum mismatch` for the first two and `taken on` the other architecture for the third
