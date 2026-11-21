## MODIFIED Requirements

### Requirement: Templates keep no time
In a cell spawned from a template, the sleeps of the template's threads
SHALL be over, their futex waits with a timeout SHALL have timed out, and
their waits on descriptors with a timeout (`poll`, `select`,
`epoll_wait`) SHALL have timed out: such a call SHALL return what is
ready, or nothing, without waiting again. Futex waits and waits on
descriptors without a timeout SHALL still wait.

#### Scenario: Template of a threaded program
- **WHEN** cells are spawned from a template of a multi-threaded program frozen after its warm-up
- **THEN** each runs all its threads to the same answer as a cold run

#### Scenario: Template frozen in a timed wait on descriptors
- **WHEN** a cell is spawned from a template frozen while its thread waits an hour in `epoll_pwait` on an empty `epoll` instance
- **THEN** the call returns 0 at once and the cell runs on to its exit
