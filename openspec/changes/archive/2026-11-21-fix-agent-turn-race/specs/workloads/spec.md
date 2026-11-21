## ADDED Requirements

### Requirement: The host sees every turn of the agent
The host running the CPython agent SHALL recognize the end of every turn,
and its decision point, from the agent's output however that output
reaches it, and SHALL suspend and resume the agent once after each turn.

#### Scenario: A pause the host does not see
- **WHEN** the agent's pause after its last turn passes before the host reads its output, so the turn's line and the decision point are read in one piece
- **THEN** the host still suspends and resumes the agent after that turn before forking it, and the agent is suspended once per turn

#### Scenario: Output split anywhere
- **WHEN** the agent's output is read in pieces split at arbitrary bytes
- **THEN** every turn's line is found exactly once, in order, and the decision point once, after the last turn
