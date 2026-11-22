## ADDED Requirements

### Requirement: Thread switches copy little
When the next thread of a cell becomes current in the same slice, the
kernel SHALL copy each of the two threads' records at most once, and
SHALL move no other thread's record when the cell has two threads. The
current thread's record SHALL stay where system calls find it, with no
added indirection or branch.

#### Scenario: Loopback between two threads
- **WHEN** `cell.loopback_rtt_insns` is measured on AArch64 under `-icount`
- **THEN** it is at or below 9,206 instructions, the two-cell figure before per-cell networks

#### Scenario: System calls unchanged
- **WHEN** `cell.syscall_insns` and `cell.pipe_rtt_insns` are measured under `-icount` on x86_64
- **THEN** they are no higher than on the parent commit
