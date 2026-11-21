## ADDED Requirements

### Requirement: Freeing a run costs per bitmap word
Freeing N contiguous frames SHALL check for a double free and mark the
frames free a 64-frame bitmap word at a time, not frame by frame, and
freeing one frame SHALL test and set its single bit. A double free inside
a run SHALL still panic, naming the lowest frame of the run that is
already free.

#### Scenario: Double free inside a run
- **WHEN** a run of 80 frames spanning three bitmap words is freed while two of its frames are already free
- **THEN** the kernel panics with a message naming the lower of those two frames

#### Scenario: Freeing a large page
- **WHEN** a cell writing a 50 MiB template's pages copies them as 2 MiB pages and exits
- **THEN** `cell.fork_touch_insns` is lower than with a frame-by-frame check
