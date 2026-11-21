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

### Requirement: Tree under control
`run`, `load` and the fork of a template SHALL take a parent, a label,
limits with their reservation mode, an egress policy and ceiling, a
policy for when the parent ends and a loopback network; `list` SHALL give
each cell's parent, domain root and label, and `rhizo ps --tree` SHALL
show the tree.

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
