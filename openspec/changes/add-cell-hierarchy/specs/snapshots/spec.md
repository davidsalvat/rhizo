# Spec Delta

## MODIFIED Requirements

### Requirement: Snapshot format
A snapshot SHALL encode to a versioned byte sequence (format version 13:
11 adds the tree state, 12 handles, 13 views) tagged with its
architecture, independent of in-memory layouts, and decoding SHALL reject
a sequence that is truncated, corrupted, of another version or of another
architecture with an error naming the reason.

#### Scenario: Wrong architecture
- **WHEN** a snapshot taken on one architecture is decoded on the other
- **THEN** decoding fails with an architecture error and no cell is created

#### Scenario: Deterministic encoding
- **WHEN** the same cell state is encoded twice
- **THEN** both byte sequences are identical

## ADDED Requirements

### Requirement: Tree state in snapshots
A snapshot SHALL hold the cell's limits and reservation mode, its policy
for when its parent ends, its egress policy and ceiling, whether it
shares a loopback network, its identifier in its domain and its label.
Resumed in the kernel that took it, the cell SHALL come back under its
parent; if the parent ended, that policy SHALL already have applied.

#### Scenario: Policy across suspend
- **WHEN** a child with egress `model` and a 16 MiB limit is suspended and resumed
- **THEN** it is under the same parent, with the same egress and limit

### Requirement: Handles in snapshots
A snapshot SHALL hold one record per handle (kind, rights, label, boot
identity and a key derived from a secret no snapshot holds) and the
messages queued on its endpoints. Resumed by the boot that took it, a
handle SHALL come back only if its object lives and it was not revoked;
otherwise an endpoint reads as reset and other handles are dropped.

#### Scenario: Revoked while suspended
- **WHEN** a cell holding two channel endpoints is suspended, one grant is revoked, and the cell is resumed
- **THEN** the other endpoint carries messages queued meanwhile, and the revoked one reads end of file

### Requirement: Loaded snapshots hold no authority
A snapshot loaded from bytes SHALL be placed where the host says, SHALL
get the intersection of its egress and ceiling with its new parent's and
limits that fit its new parent's free budget, and SHALL hold no handle but
those the host binds by label to objects of this kernel.

#### Scenario: Forged authority
- **WHEN** a snapshot file is changed to claim egress `full`, a 4 GiB limit and handles with every right, and is loaded under a parent with egress `none`
- **THEN** the cell has egress `none`, limits within its parent's budget, and no handle
