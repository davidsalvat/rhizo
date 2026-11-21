# Spec Delta

## ADDED Requirements

### Requirement: Static chat agent
A statically linked chat agent (`cells/c/chat.c`) SHALL hold the same
conversation as `agent_chat.py` against an OpenAI-compatible model,
running file tools itself and adding the `python` module on its first
snippet, which it sends over the module's `repl:python` channel.
`rhizo chat --agent static|python` SHALL choose it.

#### Scenario: Recorded chat
- **WHEN** the static chat agent runs the recorded chat against the fake model
- **THEN** its conversation equals the transcript, and the `python` module is added at the first snippet only
