# Spec Delta

## ADDED Requirements

### Requirement: Freed frames are reused first
The frame allocator SHALL hand out frames freed below its search point before
frames that were never handed out.

#### Scenario: Reuse after free
- **WHEN** frames are freed below the most recently handed-out frame
- **THEN** the next allocation returns the lowest of them
