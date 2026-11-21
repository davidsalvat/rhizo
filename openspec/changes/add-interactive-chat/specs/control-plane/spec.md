# Spec Delta

## ADDED Requirements

### Requirement: Standard input from the host
A cell the host starts with input SHALL read, on descriptor 0, a pipe of
64 KiB whose only write end the host holds, outside the cell's
descriptors. A cell started without input SHALL read end of file on
descriptor 0, as before.

#### Scenario: Descriptor 0 of a chat cell
- **WHEN** the host starts the chat agent with input
- **THEN** the agent blocks reading its standard input until the host writes to it

### Requirement: Input requests
`input` SHALL write bytes into a running cell's standard input whole or
not at all, and `close-input` SHALL close the host's end so that the cell
reads end of file once the rest is read; a thread blocked reading or
polling descriptor 0 SHALL wake. Input SHALL be refused for a cell that is
not running, has no input from the host, or closed it on either side, and
when its unread input leaves too little room.

#### Scenario: A blocked reader wakes
- **WHEN** a cell started with input blocks reading descriptor 0 and the host sends a line with `input`
- **THEN** the cell reads that line and goes on

#### Scenario: End of input
- **WHEN** the host sends `close-input` to such a cell
- **THEN** it reads what was left, then end of file, and further input is refused

### Requirement: Input across suspend and fork
A suspended cell SHALL keep its unread input and its open host end across
resume. Each branch of a fork, and each cell spawned from a template,
SHALL get its own copy of the input, holding the bytes unread at that
moment and written only when the host addresses that cell.

#### Scenario: Branches have their own input
- **WHEN** a cell with unread input is forked into two and the host writes a line into the first child only
- **THEN** both children read the unread bytes, and only the first reads the line

#### Scenario: Input across suspend
- **WHEN** a cell with unread input is suspended and resumed
- **THEN** it reads the unread bytes, and the host's next input reaches it

### Requirement: Bounded input requests
The protocol's version SHALL be 2. An `input` request carrying more than
64 KiB SHALL be refused as too large, and its decoding, like that of every
request, SHALL never panic nor allocate more than the frame's bytes.

#### Scenario: Oversized or malformed input
- **WHEN** the kernel receives an `input` request of 64 KiB plus one byte, or one whose length exceeds its payload
- **THEN** it refuses it and keeps serving

### Requirement: Prompt output
A watched cell's output SHALL reach the host as soon as the cell waits with
output not yet sent, and at least every 10 ms while it runs.

#### Scenario: Answer then wait
- **WHEN** a watched cell prints a line and then blocks reading its input
- **THEN** the host receives the line without waiting for the 10 ms period

### Requirement: Chat
`rhizo chat --llm URL --model NAME` SHALL start the chat agent in a new
cell with input from the host, and `rhizo chat N` SHALL talk to cell `N`
again, resuming it if suspended. Each line typed SHALL go to the current
branch's input, and its answer SHALL be shown as it arrives, then the
turn's time split into the model's and Rhizo's. The end of the typed input
SHALL close the agent's input. With input that is not a terminal, each
line SHALL be echoed instead of a prompt.

#### Scenario: A conversation with branches, suspended and resumed
- **WHEN** the `control` test step types into `rhizo chat` against the fake model two messages, `/fork 2`, `/branches`, `/switch` to a branch, a message and `/suspend`, then runs `rhizo chat` on that branch, types a message and closes its input
- **THEN** `/branches` lists three running cells, `rhizo chat N` says it resumed the branch, every turn prints its model and Rhizo times, the agent exits with 0, and the conversation equals the recorded transcript on x86_64 and AArch64

### Requirement: Chat commands
A chat SHALL offer `/fork K` (fork the current branch into K and stay on
it), `/branches` (the session's branches with their cells and states),
`/switch N` (talk to cell N, resuming it if suspended), `/suspend`
(suspend the current branch and leave), `/exit` (end it and leave) and
`/help`.

#### Scenario: Chat argument errors
- **WHEN** `rhizo chat` has neither a cell nor both `--llm` and `--model`, or a typed command is unknown or malformed
- **THEN** `rhizo` refuses it with a message, and in a chat the conversation goes on
