# Spec Delta

## ADDED Requirements

### Requirement: Accelerator section
A snapshot (format version 15) SHALL contain, for each of the cell's model
sessions, its label, model fingerprint, token ids, sampling state and its
KV frames by reference like other pages. A version 14 snapshot SHALL read
as having no sessions.

#### Scenario: Round trip
- **WHEN** a cell with two sessions is suspended, encoded, decoded and resumed
- **THEN** both sessions continue with the same tokens, and their KV frames are shared with the running copy rather than duplicated

### Requirement: Loaded snapshots carry no accelerator authority
A snapshot loaded from a file SHALL get no accelerator handle from its
contents. Its sessions SHALL be restored only for labels the host binds
to a handle it holds; other sessions SHALL read as closed and their KV
frames SHALL be dropped.

#### Scenario: Forged session
- **WHEN** a file claims a session on `gpu0` and is loaded without a binding
- **THEN** the cell runs, its session endpoint reads as closed, and the broker receives nothing
