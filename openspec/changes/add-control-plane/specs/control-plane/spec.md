# Spec Delta

## ADDED Requirements

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
