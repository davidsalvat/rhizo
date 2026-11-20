# Spec Delta

## ADDED Requirements

### Requirement: Calls CPython uses
The kernel SHALL implement the calls a static CPython makes at start-up
and while answering a request that earlier milestones did not need, with
Linux semantics: `time` on x86_64 (seconds since the epoch, also stored
through a non-null pointer, `-EFAULT` when that pointer is not writable).

#### Scenario: Seconds since the epoch
- **WHEN** an x86_64 cell calls `time` with a pointer to writable memory
- **THEN** it receives the real-time clock's whole seconds, and the same value is stored at the pointer
