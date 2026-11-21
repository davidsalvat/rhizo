# Spec Delta

## MODIFIED Requirements

### Requirement: Files are private
A cell SHALL NOT see or change another cell's files, except through a view
granted to it. Renaming, creating or removing a file SHALL change only the
caller's own tree and the views it shares.

#### Scenario: Two cells, same path
- **WHEN** two cells write different contents to the same path
- **THEN** each reads its own contents

#### Scenario: Renames do not signal
- **WHEN** a cell renames, creates and removes files in a loop while another cell with no view of its files lists and stats the same paths in its own tree
- **THEN** the second cell's listings never change

## ADDED Requirements

### Requirement: Snapshot views
A read-only snapshot view SHALL give the receiver, mounted at the path the
grant names, a copy-on-write copy of a directory subtree as it was at the
grant, sharing file blocks without copying them; later changes on either
side SHALL NOT reach the other.

#### Scenario: Dataset handed over
- **WHEN** a cell grants a snapshot view of a 16 MiB directory and then changes a file in it
- **THEN** the receiver reads the old contents, writes fail with `EROFS`, and free memory dropped by less than 1 MiB

### Requirement: Live views
A live view SHALL share a directory subtree between its owner and the
receiver, read-only or read-write as granted: each SHALL see the other's
changes at once, and data written through it SHALL be charged to the
owner's memory limit.

#### Scenario: Shared work directory
- **WHEN** a parent grants a child a read-write live view and the child writes a file there
- **THEN** the parent reads it at once, and a read-only receiver of the same view reads it and cannot write

### Requirement: Views do not escape
A view SHALL reach only its subtree: `..` at its root SHALL stay there,
and `rename` or `link` across its boundary SHALL fail with `EXDEV`. A
revoked view SHALL read as an empty directory and descriptors opened
through it SHALL fail with `EIO`.

#### Scenario: Climbing out
- **WHEN** a receiver opens `../../secret` from the root of a view and renames a file of the view into its own tree
- **THEN** the open finds nothing outside the view and the rename fails with `EXDEV`
