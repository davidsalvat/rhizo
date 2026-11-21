# Spec Delta

## MODIFIED Requirements

### Requirement: Fork cost independent of memory size
A copy that stays in its source's domain (a fork of a cell under a
parent, or a copy of a template the host published) SHALL NOT copy pages
or leaf page tables; it SHALL allocate only the copy's root table. A copy
that is the root of a new domain (a fork of a root, or a copy of an
unpublished template as a root) SHALL be a full copy that shares only
public objects with its source (design D21 of `add-cell-hierarchy`).

#### Scenario: Large cell
- **WHEN** a cell with 50 MiB of memory under a parent is forked, or a copy of a published template holding 50 MiB is made
- **THEN** exactly one page-table frame is allocated

#### Scenario: Fork of a root
- **WHEN** a root with 50 MiB of memory is forked
- **THEN** the copy shares no page with it but those of public objects, and the source's later writes copy nothing

## ADDED Requirements

### Requirement: Where copies go
A copy made by the host or an ancestor (a fork, or a spawn from a
template) SHALL be a sibling of its source, under its parent or where the
host says, with the source's limits carved again from that parent's
budget (all copies or none), and the source's egress policy, ceiling and
policy for when its parent ends. A copy SHALL start with no children.

#### Scenario: Fork beyond the budget
- **WHEN** a child of 16 MiB is forked into three under a parent with 40 MiB free
- **THEN** the fork is refused, no copy is made, and the parent's free budget is unchanged

### Requirement: What a copy holds
A copy SHALL hold handles derived from its source's, revoked with them,
except control handles on the source's descendants, which it SHALL NOT
hold. A channel endpoint whose peer is outside the copy SHALL read as
reset in it; a channel both of whose ends the source holds SHALL be
copied with its messages.

#### Scenario: Channel to a tool cell
- **WHEN** an agent holding a channel to a tool cell and a cell handle on it with the message right is forked
- **THEN** the copy reads end of file on the old channel, can open a new channel through its handle, and the agent's channel still works
