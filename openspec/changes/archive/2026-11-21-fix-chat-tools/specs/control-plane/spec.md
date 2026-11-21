# Spec Delta

## MODIFIED Requirements

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

## ADDED Requirements

### Requirement: Lines typed during a turn
A line typed while `rhizo chat` works on another SHALL wait for it to end,
and the chat SHALL say at once that it is queued. A prompt SHALL be shown
once per line, never in front of a line already typed.

#### Scenario: Two lines typed at once
- **WHEN** two messages are typed at once into `rhizo chat`
- **THEN** it prints `[rhizo] queued for the next turn` once, answers the first message, then the second
