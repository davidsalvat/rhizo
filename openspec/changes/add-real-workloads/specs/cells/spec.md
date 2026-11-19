# Spec Delta

## ADDED Requirements

### Requirement: Floating-point state is per cell
Each cell SHALL have its own floating-point and SIMD registers, kept across
preemption, system calls, suspend/resume and fork, and never visible to
another cell.

#### Scenario: Two FP cells
- **WHEN** two cells hold different values in FP/SIMD registers across yields
- **THEN** each finds its own values

#### Scenario: FP across a snapshot
- **WHEN** a cell computing with FP/SIMD registers is suspended and resumed repeatedly
- **THEN** its result is the same as without suspension
