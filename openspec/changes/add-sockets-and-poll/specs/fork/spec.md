# Spec Delta

## ADDED Requirements

### Requirement: Pipes are forked copy-on-write
A forked cell SHALL start with copies of its parent's pipes (with their
data), event counters and `epoll` instances; afterwards each SHALL see only
its own reads and writes.

#### Scenario: Diverging pipe
- **WHEN** a cell with unread bytes in a pipe is forked and parent and child each read the pipe
- **THEN** both read the same bytes
