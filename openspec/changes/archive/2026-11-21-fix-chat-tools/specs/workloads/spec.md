# Spec Delta

## MODIFIED Requirements

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

## ADDED Requirements

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
