# Spec Delta

## ADDED Requirements

### Requirement: Pipes are forked copy-on-write
A forked cell SHALL start with copies of its parent's pipes (with their
data), event counters and `epoll` instances; afterwards each SHALL see only
its own reads and writes.

#### Scenario: Diverging pipe
- **WHEN** a cell with unread bytes in a pipe is forked and parent and child each read the pipe
- **THEN** both read the same bytes

### Requirement: Copy-on-write needs no free 2 MiB block
A write to a 2 MiB page shared with another cell SHALL succeed while
4 KiB frames remain, even when no whole free 2 MiB block is left.

#### Scenario: Fragmented memory
- **WHEN** a forked cell writes to a shared 2 MiB page and memory has free 4 KiB frames but no free 2 MiB block
- **THEN** the write succeeds, the cell sees its own data, and the other cell does not see the write
