# Spec Delta

## MODIFIED Requirements

### Requirement: Suspend
The kernel SHALL suspend a live cell into a snapshot holding, for each of
its threads, its registers, FP/SIMD state, thread pointer, identifier,
clear-tid address and what it waits for (a sleep or a futex, with the time
left), and the cell's program break, mapping cursor and every mapped page
with its permissions and contents, and SHALL return all of the cell's
memory and page tables to the allocator.

#### Scenario: Memory returned
- **WHEN** a cell is suspended
- **THEN** the free memory is what it was before the cell was spawned, plus nothing held by the cell

#### Scenario: Waiting threads in a snapshot
- **WHEN** a cell with a thread blocked on a futex is suspended and resumed, and another thread then wakes the futex
- **THEN** the blocked thread wakes and the program completes as without suspension

## ADDED Requirements

### Requirement: Threaded cells across snapshots
Suspending and resuming a multi-threaded cell SHALL be invisible to it:
each thread continues where it was, in the same scheduling order.

#### Scenario: Threads after every step
- **WHEN** a multi-threaded glibc program using mutexes and condition variables is suspended and resumed after every scheduling step until it exits
- **THEN** its complete output and exit status are identical to an uninterrupted run
