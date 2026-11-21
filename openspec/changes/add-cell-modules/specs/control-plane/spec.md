# Spec Delta

## ADDED Requirements

### Requirement: Module commands
`rhizo` SHALL offer `module load` (verify and load a signed module),
`module list`, `module build [--twice]` (start a builder for a lockfile),
`module review`, `module approve` (sign with the operator's key and add it
to the catalogue), `module request` (show lockfiles agents asked for) and
`run --modules FILE` (a cell's module policy).

#### Scenario: Build to catalogue
- **WHEN** the operator builds a module from a lockfile, reviews it and approves it
- **THEN** a signed file appears in the catalogue, `module load` accepts it, and the builder's domain is gone

### Requirement: Approvals from the host
The kernel SHALL send the host an event for each module request whose
approver is the host, and `rhizo approvals`, `rhizo approve ID` and
`rhizo deny ID` SHALL list and answer them.

#### Scenario: Operator approves
- **WHEN** an agent asks for an `ask` module and the operator runs `rhizo approve` on its request
- **THEN** the agent receives the module's channel
