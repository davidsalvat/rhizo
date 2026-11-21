## ADDED Requirements

### Requirement: Scratch directory in the CPython cell
The CPython cell's files SHALL include an empty, writable `/tmp` directory
next to `/usr/lib/python3.13` and `/app`.

#### Scenario: A program writes a scratch file
- **WHEN** code running in the CPython cell writes `/tmp/out.txt` and reads it back
- **THEN** both succeed, and the file travels with the cell through fork and suspend
