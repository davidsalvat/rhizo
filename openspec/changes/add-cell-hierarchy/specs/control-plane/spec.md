# Spec Delta

## MODIFIED Requirements

### Requirement: Bounded input requests
The protocol's version SHALL be 4. An `input` request carrying more than
64 KiB SHALL be refused as too large, and its decoding, like that of every
request, SHALL never panic nor allocate more than the frame's bytes.

#### Scenario: Oversized or malformed input
- **WHEN** the kernel receives an `input` request of 64 KiB plus one byte, or one whose length exceeds its payload
- **THEN** it refuses it and keeps serving

#### Scenario: Malformed tree request
- **WHEN** the kernel receives a `run` naming an unknown parent, an egress rule with a prefix over 32, or a `grant` with unknown rights
- **THEN** it refuses it with the reason and keeps serving

## ADDED Requirements

### Requirement: Protocol versions on both sides
The kernel's hello SHALL name its protocol version. `rhizo` SHALL speak
the current version and the one before it, and SHALL refuse a kernel of
any other version with a message naming both versions and how to
upgrade (save the cells, `rhizo down`, `rhizo up` with the new kernel,
`rhizo snapshot upgrade`, load).

#### Scenario: Old kernel
- **WHEN** `rhizo` meets a kernel two protocol versions older
- **THEN** it talks to no cell and prints both versions and the upgrade steps

### Requirement: Tree under control
`run`, `load` and the fork of a template SHALL take a parent, a label,
limits, an egress policy and ceiling (egress `none` when not given), a
policy for when the parent ends and a CPU set; `list` SHALL give each
cell's parent, domain root and label, and `rhizo ps --tree` SHALL show
the tree. `rhizo chat` and agent templates SHALL pass `--egress model`.

#### Scenario: A small tree
- **WHEN** the host runs an agent, two helpers under it with egress `none`, and lists the cells
- **THEN** `rhizo ps --tree` shows both helpers indented under the agent with their egress and limits

### Requirement: Tree operations from the host
The host SHALL be able to re-parent a cell or make it a root, narrow or
set its egress and limits, kill, suspend or resume a whole subtree, and
grant, list and revoke handles (`rhizo reparent`, `policy`, `grant`,
`grants`, `revoke`, and `--tree` on `kill`, `suspend` and `resume`).

#### Scenario: Grant and revoke from the host
- **WHEN** the host grants a channel between two cells, they exchange a message, and the host revokes it
- **THEN** the message arrives, and after the revocation both ends read end of file

### Requirement: Scoped host access
`rhizo scope N --socket PATH` SHALL serve on PATH requests limited to
`N`'s subtree: the kernel SHALL refuse any whose target is outside it,
that moves a cell out of it, or that widens a policy or a limit.

#### Scenario: Out of scope
- **WHEN** a client of a socket scoped to cell 3 kills cell 3's child, then asks to kill cell 9 of another domain
- **THEN** the first succeeds and the second is refused, and cell 9 runs on

### Requirement: Audit log
The kernel SHALL send the host an audit record, with a sequence number,
for each grant, revocation, re-parent, promotion, policy or limit change,
cell creation and end, egress denial and refused tree request; the host
SHALL append each to its audit log with the hash of the record before it,
and `rhizo audit verify` SHALL detect a changed, removed or missing
record.

#### Scenario: Tampered log
- **WHEN** a grant, a revocation and an egress denial are logged and one record is then edited
- **THEN** `rhizo audit tail` shows the three, and `rhizo audit verify` names the edited record

### Requirement: Audit records are never lost silently
The kernel SHALL keep at most 64 KiB of audit records the host has not
taken, never block a cell for them, and SHALL report how many it dropped
in the next record; the host SHALL report a gap in sequence numbers.

#### Scenario: Host away
- **WHEN** the host stops reading while cells make 100,000 grants, then reads again
- **THEN** cells were never blocked, and the log holds a record of how many records were dropped
