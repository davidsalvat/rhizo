# Spec Delta

## ADDED Requirements

### Requirement: Files in snapshots
A snapshot SHALL contain the cell's file system, open descriptors and
working directory, and a resumed cell SHALL find them as they were.

#### Scenario: Open file across suspend
- **WHEN** a cell with a file open is suspended and resumed between two reads
- **THEN** the second read continues at the same offset with the same data
