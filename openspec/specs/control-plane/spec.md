# control-plane Specification

## Purpose

How the host drives a running kernel: a control kernel that keeps a table
of cells, the versioned protocol on its serial line, the `rhizo`
command-line tool (`up`, `run`, `ps`, `fork`, `suspend`, `resume`,
`attach`, `kill`, `down`, `chat`), standard input sent to cells, and the
interactive chat with an agent in a cell.

## Requirements

### Requirement: Control kernel
A `control` kernel build SHALL boot, say hello on its control channel, and
then serve the host's requests until asked to power off: run, list,
suspend, resume, fork and kill cells, stream a cell's output, write a file
into a running cell, report memory and cells. It SHALL decide nothing by
itself, and every reply SHALL carry the time the kernel took, in
nanoseconds.

#### Scenario: Power off
- **WHEN** the host asks the kernel to power off
- **THEN** the kernel replies, and the VMM ends

### Requirement: Cells under control
A cell SHALL run from an executable and a file set the kernel holds, with
the arguments, environment and added files the request gives. It SHALL keep
its identifier across suspend and resume; killing a cell that ended SHALL
forget it.

#### Scenario: Files and environment
- **WHEN** the host runs `python` with an added file `/app/answer.py`, that path as its argument and a variable in its environment
- **THEN** the cell runs the file, sees the variable, and exits with 0

#### Scenario: Through every operation
- **WHEN** the host runs the idle cell, suspends it, resumes it, forks it into two, kills the three and lists the cells
- **THEN** each request succeeds, the list shows the suspended cell while suspended and the three as killed after, and every reply carries a time

#### Scenario: Output and exit
- **WHEN** the host runs `hello` and streams its output
- **THEN** it receives what the cell printed, then that the cell exited with 0

#### Scenario: Refusals
- **WHEN** a request names a cell that does not exist, or an executable the kernel does not hold, or asks to resume a running cell
- **THEN** the kernel refuses it with the reason and its time, and keeps serving

### Requirement: Control channel on every VMM
The control channel SHALL be the VM's serial line, both ways, on QEMU,
Firecracker and Cloud Hypervisor and on both architectures; the kernel's
console text SHALL share it, and a cell's output SHALL reach the host only
in the protocol's frames.

#### Scenario: Boot on the default VMM
- **WHEN** `rhizo up` boots the control kernel on QEMU (`microvm` on x86_64, `virt` on AArch64)
- **THEN** the host receives the kernel's hello on the VMM's standard output, after its console text

### Requirement: Control protocol
Frames SHALL start with the byte 0x01 and `Z`, then a version, a kind, a
request identifier and a payload length (little endian), then the payload;
replies and output SHALL carry the identifier of the request they answer.
Decoding SHALL never panic and never allocate more than the bytes received:
a frame of another version SHALL be refused and skipped, a payload over
1 MiB refused, a malformed payload refused, and the next frame found after
any text or damage that leaves its header readable.

#### Scenario: Round trip
- **WHEN** any request or message is encoded, with any identifier, between any console text
- **THEN** decoding gives back the same text, and the same requests or messages with their identifiers

#### Scenario: Hostile input
- **WHEN** arbitrary bytes, or a valid frame with one byte changed, are decoded
- **THEN** nothing panics, and a valid frame after them is found

### Requirement: The rhizo tool
`rhizo` SHALL boot a VM with the control kernel (`up`, choosing the VMM,
architecture, memory, CPUs and acceleration), keep a state that later
commands use to reach it, and offer `run` (templates: `python-agent`,
`python`, any program the kernel holds), `ps`, `stats`, `fork`, `suspend`,
`resume`, `attach`, `kill` and `down`, each printing the kernel's time and
the round trip the host measured. Several commands SHALL be able to talk to
the same VM at once.

#### Scenario: Argument errors
- **WHEN** a command is unknown, misses its cell, or `run python-agent` misses `--llm` or `--model`
- **THEN** `rhizo` fails with a message and talks to no VM

#### Scenario: Up and down
- **WHEN** `rhizo up` then `rhizo down` run
- **THEN** the first returns once the kernel answers, and the second once the VM and its state are gone

### Requirement: The agent through the control plane
`rhizo run python-agent --llm URL --model NAME` SHALL run the CPython agent
against that model (a loopback address rewritten to the host as the guest
sees it), suspend and resume it after every turn, fork it into three
branches at its decision point with each branch's number in `/branch`, end
the agent, print each step with the model's time apart from Rhizo's, and
end with `rhizo-demo done` when every branch exited with 0; it SHALL fail
otherwise.

#### Scenario: Transcript against the fake model
- **WHEN** the `control` test step runs the agent with `--transcript` against the host's fake model
- **THEN** its lines equal the recorded transcript, with one suspend per turn and one fork

### Requirement: Demo wrapper
`cargo xtask demo agent` SHALL build the control kernel and `rhizo` and run
the agent through `rhizo up`, `rhizo run python-agent` and `rhizo down`.

#### Scenario: Result of the demo
- **WHEN** `rhizo run python-agent` fails or succeeds
- **THEN** `cargo xtask demo agent` fails or succeeds with it, and the VM is down either way

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
- **WHEN** the `control` test step types into `rhizo chat` against the fake model two messages, `/fork 2`, `/branches`, `/switch` to a branch, a message and `/suspend`, then runs `rhizo chat` on that branch, types a message, then two messages at once, and closes its input
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

### Requirement: Lines typed during a turn
A line typed while `rhizo chat` works on another SHALL wait for it to end,
and the chat SHALL say at once that it is queued. A prompt SHALL be shown
once per line, never in front of a line already typed.

#### Scenario: Two lines typed at once
- **WHEN** two messages are typed at once into `rhizo chat`
- **THEN** it prints `[rhizo] queued for the next turn` once, answers the first message, then the second
