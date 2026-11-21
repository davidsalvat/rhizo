# Spec Delta

## ADDED Requirements

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
SHALL be a result the model reads.

#### Scenario: A snippet that does not end
- **WHEN** a snippet prints and then loops forever
- **THEN** it is stopped after its time limit, and the result holds what it printed and that it was stopped
