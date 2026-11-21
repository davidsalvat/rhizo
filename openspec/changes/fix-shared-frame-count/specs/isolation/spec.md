## MODIFIED Requirements

### Requirement: Hostile calls get answers, not crashes
Whatever call number and arguments a cell passes, the kernel SHALL answer
with a value or a negated Linux `errno`, SHALL NOT panic, and SHALL leave
kernel memory unchanged; once the cell ends, every frame it held SHALL be
free again. No count of references to a frame that a cell can grow
(mapping one file page again and again) SHALL overflow or panic: past its
bound, the cell gets a copy of the page instead.

#### Scenario: Random calls
- **WHEN** a cell makes 20000 system calls with numbers and arguments from a fixed-seed generator, next to another cell
- **THEN** every result is a value or an errno, both cells finish, and running it again leaves memory as it was

#### Scenario: One page mapped without end
- **WHEN** a cell maps the first page of a file of its own 70000 times, next to another cell
- **THEN** every mapping reads the file's bytes, both cells finish, the kernel keeps running, and every frame comes back
