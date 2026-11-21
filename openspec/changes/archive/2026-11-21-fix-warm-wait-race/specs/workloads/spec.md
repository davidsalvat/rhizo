## ADDED Requirements

### Requirement: Templates frozen at their pause
A host that freezes a cell as a template after a line it prints (`warm`,
`ready`, `start`) SHALL freeze it at the pause that follows that line,
before the cell runs past it, however long the pause and however late the
host reads the cell's output; a cell that ends or prints past the line
before pausing SHALL be reported as such instead of being waited for.

#### Scenario: A pause the host does not see
- **WHEN** the pause after `warm` (a 1 ms sleep) is already over when the cell's sleep blocks, as on a loaded host
- **THEN** the host still freezes the template asleep after `warm`, and every cell forked from it prints the whole answer

#### Scenario: Output split anywhere
- **WHEN** the cell's output is read in pieces split at arbitrary bytes
- **THEN** the line is found once all of it has been read, never before, and every byte after it is counted
