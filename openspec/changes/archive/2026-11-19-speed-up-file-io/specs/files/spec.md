# Spec Delta

## ADDED Requirements

### Requirement: File data takes its own size in memory
A cell's file data SHALL take at most its size in kernel memory plus a small
bookkeeping overhead (under 1/16), so the 64 MiB bound on file data is also
a bound on the memory it uses.

#### Scenario: A 16 MiB file
- **WHEN** a cell writes a 16 MiB file and holds it
- **THEN** free memory drops by less than 17 MiB, cell memory included
