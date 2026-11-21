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

### Requirement: Key and proxy commands
`rhizo` SHALL offer `keys generate --purpose modules|revocation`, `keys
rotate`, `keys revoke`, `module resign`, `module check` (loaded or
catalogue modules that are revoked or about to expire), `module unload
[--kill]` and `proxy` (the package proxy, also started by `module build`).

#### Scenario: Revoking a loaded module
- **WHEN** the operator revokes the digest of a module loaded in a running VM and runs `rhizo module check`, then `rhizo module unload --kill`
- **THEN** the check names the module, no new add of it succeeds, and its running copies end
