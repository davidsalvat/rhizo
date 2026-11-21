# workloads Specification

## Purpose
Defines running real Linux programs, built with a real libc, as cells and
holding them to the behavior they have on Linux.

## Requirements

### Requirement: Real libc programs
Static programs built with the system's C library SHALL run as cells
unchanged and produce the same output and exit status as on Linux.

#### Scenario: glibc program
- **WHEN** a static glibc program that formats numbers, sorts, allocates large and small blocks and uses floating point runs as a cell
- **THEN** its output and exit status equal those of the same binary on Linux

### Requirement: Warm templates of real programs
A real program frozen after its warm-up SHALL serve each new request as a
fork that does not repeat the warm-up.

#### Scenario: Warm workload
- **WHEN** cells are spawned from a template of `libc_warm` frozen after it printed `warm`
- **THEN** each prints only its answer, the same as a cold run's answer

### Requirement: A real interpreter
MicroPython SHALL run as a cell, import a module from the cell's files and
print what the same binary prints on Linux; cells forked from a template
frozen after its warm-up SHALL answer without warming up again.

#### Scenario: Python request
- **WHEN** MicroPython runs `rhizo_demo.main()` with the module in the cell's `/lib`
- **THEN** it prints the output recorded from Linux and exits with 0

#### Scenario: Warm Python template
- **WHEN** cells are forked from MicroPython frozen after it printed `warm`
- **THEN** each prints only the answer and exits with 0

### Requirement: Threaded libc programs
A static glibc program that creates threads with `pthread_create`
(default attributes, so glibc's own stacks and guard pages), synchronizes
them with mutexes and condition variables, and joins them SHALL run as a
cell and print what it prints on Linux.

#### Scenario: Producers and consumers
- **WHEN** `libc_threads` runs its producer, consumer and counter threads as a cell
- **THEN** its output and exit status equal those of the same binary on Linux

### Requirement: CPython in a cell
A static build of a pinned CPython release SHALL run as a cell, unmodified,
with the standard-library modules it uses in the cell's files, and print
what the same executable prints on Linux with the same files and an empty
environment; cells forked from a template frozen after its warm-up SHALL
answer without warming up again. Where CPython's source is not given, the
checks SHALL report that they did not run.

#### Scenario: Agent request
- **WHEN** CPython runs `/app/agent_demo.py` in a cell holding the packed standard library under `/usr/lib/python3.13`
- **THEN** it prints the output recorded from Linux, nothing else, and exits with 0

#### Scenario: Warm CPython template
- **WHEN** cells are forked from CPython frozen after it printed `warm`
- **THEN** each prints only the answer and exits with 0

#### Scenario: Same behavior on Linux
- **WHEN** the differential check runs the same CPython on the same files on a Linux host of the same architecture
- **THEN** it prints the recorded output and nothing on standard error

#### Scenario: Not built
- **WHEN** `RHIZO_CPYTHON_SRC` is not set
- **THEN** the in-kernel test and the differential check say CPython was not built, and pass

### Requirement: Parallel threaded programs
A static glibc program that splits a computation between `pthread_create`
threads, which take work and add their results under a mutex and signal a
condition variable the first thread waits on, SHALL run as a cell, print
what it prints on Linux, its threads running on several CPUs at once when
the CPUs are idle.

#### Scenario: Parallel sum against Linux
- **WHEN** `parsum` runs as a cell
- **THEN** its output and exit status equal those of the same binary on Linux

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

### Requirement: A conversational agent
The CPython agent SHALL have a conversation mode (`agent_chat.py URL
MODEL`) that reads one user message per line from its standard input,
keeps the whole conversation in the cell, answers each message through the
OpenAI-compatible API, shows answers whole (with or without tool calls),
reconnects once when its connection to the model is gone, ends each answer
with a line giving the model's time and its own, and ends at the end of its
input. The scripted agent SHALL be unchanged.

#### Scenario: Tool call then answer
- **WHEN** the fake model answers a message with a snippet call and then with text quoting the snippet's output
- **THEN** the agent shows the call, the snippet's output and the answer, then its timing line

### Requirement: Tools inside the cell
The conversational agent SHALL run the tools the model calls inside the
cell: a Python snippet (output capped at 4000 characters, stopped after a
time limit), read a file, write a file, list a directory; a tool's error
SHALL be a result the model reads. As in the Python REPL, when a snippet's
last statement is an expression, its value's `repr` SHALL follow what the
snippet printed, unless the value is None.

#### Scenario: A snippet that does not end
- **WHEN** a snippet prints and then loops forever
- **THEN** it is stopped after its time limit, and the result holds what it printed and that it was stopped

#### Scenario: A snippet ending in an expression
- **WHEN** the fake model calls the snippet tool with `import math`, `result = 18 * 8`, `sqrt_result = math.sqrt(result)` and `sqrt_result` on four lines
- **THEN** the tool's result is `12.0`, and the agent shows it, on x86_64 and AArch64

### Requirement: Scratch directory in the CPython cell
The CPython cell's files SHALL include an empty, writable `/tmp` directory
next to `/usr/lib/python3.13` and `/app`.

#### Scenario: A program writes a scratch file
- **WHEN** code running in the CPython cell writes `/tmp/out.txt` and reads it back
- **THEN** both succeed, and the file travels with the cell through fork and suspend

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

### Requirement: Packages from PyPI in a cell
A dynamically linked build of the pinned CPython SHALL run as a cell with
pinned, checksummed wheels from PyPI installed unmodified in its
`site-packages` (numpy, whose extensions it opens with `dlopen`, and
requests with what it needs), and print what it prints on Linux with the
same files and an empty environment. Where it is not built, the checks
SHALL report that they did not run.

#### Scenario: numpy computes
- **WHEN** the packages' CPython runs `app/numpy_demo.py` in a cell (array operations, a linear solve, a ufunc, a matrix product)
- **THEN** it prints the output recorded from Linux and exits with 0, and once the kept file pages are dropped every frame and heap allocation is back

#### Scenario: requests reaches the host
- **WHEN** the packages' CPython runs `app/requests_demo.py` in a cell with the URL of the host's fake model, through the network card
- **THEN** it prints the status, content type, detected encoding and tool call recorded from Linux and exits with 0

#### Scenario: A template that imported numpy
- **WHEN** a cell stopped after `import numpy` is suspended and resumed from the snapshot's bytes, forked, and frozen as a template that new cells are forked from
- **THEN** every copy prints the computation and exits with 0

#### Scenario: Same behavior on Linux
- **WHEN** the differential check runs both scripts natively from the top of the packed tree with an empty environment, requests against the host's fake model
- **THEN** each prints the recorded output and exits with 0

#### Scenario: A wheel that is not the pinned one
- **WHEN** a downloaded wheel's SHA-256 differs from its pin
- **THEN** the build fails, naming the URL, the expected and the actual checksum

### Requirement: Tool calls written as text
When a reply of the model has no tool calls, the conversational agent SHALL
run, as a tool call, the first JSON object in its text that parses, names
one of its tools and has an object of arguments, and SHALL NOT show that
text as an answer. Any other text SHALL be the answer.

#### Scenario: A call in the message's text
- **WHEN** the fake model answers `Sure {"name": "run_python", "arguments": {"code": "6 * 7"}}` with no tool calls
- **THEN** the agent shows the call and its result `42`, then the model's next answer, and does not show the JSON as an answer

#### Scenario: Text that is not a call
- **WHEN** a reply's text holds no JSON object, or one naming no tool of the agent, or one whose arguments are not an object
- **THEN** the agent shows the text as the answer and runs no tool

### Requirement: Code written at run time
A statically linked C program that writes machine code into memory it maps
writable and executable, runs it, rewrites it and runs it again, also from
two threads at once, SHALL print in a cell what it prints on Linux, also
when forked, suspended and resumed between its steps.

#### Scenario: A small JIT
- **WHEN** `libc_jit` runs in a cell, plain, forked at `ready`, or suspended and resumed there
- **THEN** every copy prints its expected output and exits with 0
