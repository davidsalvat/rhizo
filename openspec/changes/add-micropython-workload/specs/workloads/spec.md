# Spec Delta

## ADDED Requirements

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
