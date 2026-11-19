# Spec Delta

## ADDED Requirements

### Requirement: Comparison table
The README benchmark section SHALL include a table of cell operations next
to the Firecracker and Linux-process measurements of the same run, showing
"—" for anything not measured in that run.

#### Scenario: Missing measurement
- **WHEN** the results have no Firecracker restore figure
- **THEN** the Firecracker column of the resume row shows "—"

### Requirement: Complete results from CI
The CI benchmark job SHALL publish its complete results, so the README can
be regenerated from them exactly.

#### Scenario: Large results
- **WHEN** the results JSON is larger than one annotation
- **THEN** it is published in numbered pieces that concatenate to the file
