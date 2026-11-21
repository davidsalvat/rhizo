# Spec Delta

## MODIFIED Requirements

### Requirement: Suspend
The kernel SHALL suspend a live cell into a snapshot holding, for each of
its threads, its registers, FP/SIMD state, thread pointer, identifier,
clear-tid address and what it waits for (a sleep or a futex, with the time
left), and the cell's program break, mapping cursor, quotas and CPU time
used, and every mapped page with its permissions and contents, and SHALL
return all of the cell's memory and page tables to the allocator.

#### Scenario: Memory returned
- **WHEN** a cell is suspended
- **THEN** the free memory is what it was before the cell was spawned, plus nothing held by the cell

#### Scenario: Waiting threads in a snapshot
- **WHEN** a cell with a thread blocked on a futex is suspended and resumed, and another thread then wakes the futex
- **THEN** the blocked thread wakes and the program completes as without suspension

#### Scenario: Quotas in a snapshot
- **WHEN** a cell with quotas that used some CPU time is suspended and its snapshot decoded
- **THEN** the restored cell has the same quotas and the same CPU time used
