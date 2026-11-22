## MODIFIED Requirements

### Requirement: Templates frozen at their pause
A host that freezes a cell as a template after a line it prints (`warm`,
`ready`, `start`) SHALL freeze it at the pause that follows that line,
before the cell runs past it, whatever else runs meanwhile; a cell that
ends or prints past the line before pausing SHALL be reported as such,
with what it printed, instead of being waited for.

#### Scenario: A pause the host does not see
- **WHEN** the pause after `warm` (a 1 ms sleep) is already over when the cell's sleep blocks, as on a loaded host
- **THEN** the host still freezes the template asleep after `warm`, and every cell forked from it prints the whole answer

#### Scenario: Output split anywhere
- **WHEN** the cell's output is read in pieces split at arbitrary bytes
- **THEN** the line is found once all of it has been read, never before, and every byte after it is counted

#### Scenario: A threaded pause next to busy cells
- **WHEN** a template's 20 ms sleep after `ready` starts while another of its threads can still run (it yields four times, then waits), and three other cells use every slice they get
- **THEN** the host freezes the template with every thread waiting before the sleep ends, and a copy of it prints only what follows the pause

#### Scenario: A pause that cannot be reached
- **WHEN** the cell prints past its line or ends before it pauses
- **THEN** the error names what the cell printed (and how it ended), and the test reports that error rather than another failure
