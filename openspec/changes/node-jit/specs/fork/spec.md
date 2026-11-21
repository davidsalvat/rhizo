## ADDED Requirements

### Requirement: Forks keep write-or-execute memory
A forked cell, and a cell spawned from a template, SHALL keep its parent's
write-or-execute pages, in their state, and its write-or-execute ranges;
afterwards each SHALL write and run only its own code.

#### Scenario: Diverging code
- **WHEN** a cell that runs code from a write-or-execute page is forked, and parent and children each write different code there and run it
- **THEN** each runs its own code

#### Scenario: Template of a JIT
- **WHEN** cells are spawned from a template frozen after it ran code from write-or-execute memory
- **THEN** each runs the template's code, and its own after rewriting it
