# Spec Delta

## ADDED Requirements

### Requirement: Ownership tree
Every cell SHALL have exactly one parent: the host, or the cell at whose
request it was created. A cell the host creates SHALL be a child of the
host or of the cell the host names. The children of the host are roots;
a root and its descendants form a domain. Suspending a cell SHALL NOT
suspend its children.

#### Scenario: Children of a cell
- **WHEN** the host runs cell A, then runs B with parent A, then suspends A
- **THEN** the tree lists A under the host and B under A, A's domain holds both, and B keeps running

### Requirement: Authority flows down
Only a cell's ancestors and the host SHALL be able to suspend, resume,
fork, freeze, kill, re-parent or narrow it, unless a handle was granted.
No operation SHALL let a cell act on itself except by ending, or on an
ancestor. A handle with the control right SHALL never be delivered to the
cell it names or to a descendant of that cell.

#### Scenario: A child turns on its parent
- **WHEN** a child asks the kernel to kill, suspend or promote its parent, its sibling or itself
- **THEN** each request is refused with `EACCES` and nothing changes

#### Scenario: Control handed to the controlled
- **WHEN** a parent sends its child, over a channel, a control handle on that child
- **THEN** the send fails with `EACCES` and the child holds no new handle

### Requirement: Re-parenting
Moving a cell to a new parent or making it a root SHALL be done only by
its parent, an ancestor or the host (or a control handle derived from
one), never by the cell or a descendant. Only the host SHALL move a cell
to another domain. The new parent SHALL NOT be in the cell's subtree, the
cell's egress SHALL become its intersection with the new parent's, and
its reservation SHALL fit the new parent's free budget, or the move fails.

#### Scenario: Moves that are refused
- **WHEN** a cell is moved under its own child, a cell moves itself, or a parent moves a child into another domain
- **THEN** each move is refused and the tree is unchanged

#### Scenario: Narrower parent
- **WHEN** a cell with egress `full` holding a connection to the host's echo server is moved under a parent with egress `none`
- **THEN** its egress becomes `none` and its next call on that connection fails with `ECONNRESET`

### Requirement: Promotion moves resources, never creates them
A cell promoted to root at a cell's request SHALL keep its subtree,
egress policy and reservation, and its former parent's limits SHALL shrink
by that reservation, so the total reserved by roots does not change.

#### Scenario: Parent promotes a child
- **WHEN** a parent with a 64 MiB limit promotes a reserved child of 16 MiB
- **THEN** the child is a root with 16 MiB, and the parent's limit reads 48 MiB

### Requirement: When a parent ends
Each cell SHALL carry a policy, set by its creator and changed only by an
ancestor or the host, for when its parent ends: `cascade`, the default,
ends it and its descendants (suspended ones discarded, templates
dropped), and `promote` makes it a root with its subtree. No cell SHALL
be left without a parent.

#### Scenario: Cascade
- **WHEN** a parent with a running, a suspended and a frozen child, one with a grandchild, exits
- **THEN** all of them end, their snapshots and template are gone, and every frame and reservation comes back

#### Scenario: Promote
- **WHEN** a parent whose child has the policy `promote` exits
- **THEN** the child keeps running as a root with its own subtree

### Requirement: Budgets carved from the parent
A child's limits on memory, CPU time, sockets and descendants SHALL be
carved from its parent's free budget when it is created: refused if they
do not fit, given back when it ends (CPU time less what it used). A cell
SHALL create children only while its limits on memory, sockets and
descendants are finite, so a subtree never holds more than its root got.

#### Scenario: Fork bomb
- **WHEN** a cell with a limit of 8 descendants and 64 MiB makes children that each try to make children without end
- **THEN** at most 8 cells are made in its subtree, the rest are refused, another domain runs undisturbed, and everything comes back when the subtree ends

### Requirement: Reserved children only
Every child's limits SHALL be reserved in full from its parent's budget
when it is made; no child SHALL draw from a pool shared with its
siblings. A child's refusals SHALL depend only on its own use. The
budget record SHALL carry a mode field, always `reserved`, so that other
modes can be added by a later change.

#### Scenario: Reserved siblings do not see each other
- **WHEN** a reserved child takes memory until refused while its reserved sibling probes how much it can take, and again with the first idle
- **THEN** the sibling gets exactly as much both times

### Requirement: Reserved roots
A root with a memory limit SHALL have it reserved: the kernel SHALL refuse
a reserved root whose limits, added to those of the other reserved roots,
exceed what cells can take, and SHALL NOT let an unlimited root or the
kernel's caches take memory a reserved root has not used. Socket and
descendant limits SHALL be admitted the same way against the kernel's
pools.

#### Scenario: Unlimited neighbour
- **WHEN** an unlimited root takes memory until refused next to a reserved root of 32 MiB
- **THEN** the reserved root can still take all of its 32 MiB

### Requirement: Everything a cell takes is charged
The memory a cell's actions take SHALL count against its memory limit:
pages and page tables, file data and names, thread and socket records,
socket queues and network buffers, channel queues, and the file pages its
domain keeps.

#### Scenario: Files count
- **WHEN** a cell with an 8 MiB memory limit writes a file until refused
- **THEN** it is refused with `ENOSPC` or `ENOMEM` having taken at most 8 MiB in all

### Requirement: No ambient authority
A new cell SHALL hold no handle but those its creator gives it, and a
loopback network of its own that no other cell shares: no channel, no
cell handle, no view, no kernel endpoint and, by default, egress `none`.

#### Scenario: A new cell looks around
- **WHEN** a cell created with no grants lists `/run/rhizo`, connects to every loopback port and opens every descriptor number up to 1023
- **THEN** it finds only an empty `inbox`, every connection is refused, and only its own descriptors open

### Requirement: Handles and rights
A handle SHALL name one kernel object with rights: on a cell observe,
input, message and control; on a channel endpoint send, receive and
carry; on a view read and write; on the kernel endpoint spawn, channel,
derive, revoke and narrow. A cell SHALL use handles only through
descriptors and entries of `/run/rhizo`; a handle's number, sent as
bytes, SHALL confer nothing.

#### Scenario: A number is not a handle
- **WHEN** a cell writes the descriptor number of its channel endpoint to another cell, which uses that number
- **THEN** the number names nothing or one of the receiver's own descriptors

### Requirement: The rhizo directory
`/run/rhizo` SHALL be kept by the kernel in the cell's own tree, with
`chan/<label>` (endpoints), `cell/<label>` (cell handles), `inbox` and
`kernel` when granted. A cell MAY list, open and connect its entries and
unlink one to drop the handle; creating, renaming or writing entries
SHALL fail with `EROFS`.

#### Scenario: Rename refused
- **WHEN** a cell renames `/run/rhizo/chan/tools`
- **THEN** it receives `EROFS` and the entry is unchanged

### Requirement: Attenuation
A cell SHALL be able to derive from a handle another holding a subset of
its rights (for a view, also a subdirectory), and no request SHALL give a
cell more rights than it holds.

#### Scenario: Asking for more
- **WHEN** a cell holding a cell handle with observe only asks to derive one with control
- **THEN** the request fails with `EACCES`

### Requirement: Transfer over channels
A handle SHALL move between cells only as `SCM_RIGHTS` on an endpoint with
the carry right; the receiver SHALL hold a handle derived from the
sender's. Descriptors of files, pipes, sockets, `epoll` instances and the
kernel endpoint SHALL NOT be sent (`EPERM`).

#### Scenario: A socket to the outside
- **WHEN** a cell with egress `full` sends a connected TCP socket to a cell with egress `none`
- **THEN** `sendmsg` fails with `EPERM` and the receiver gets nothing

### Requirement: Revocation
A cell SHALL be able to revoke every handle derived from one it holds,
and an ancestor or the host any handle granted by a cell of its subtree,
at once: a revoked endpoint SHALL read end of file and fail writes with
`EPIPE`, a revoked cell handle SHALL refuse operations with `EACCES`, and
a revoked view SHALL disappear.

#### Scenario: Revoked while passed on
- **WHEN** A grants B a channel, B passes it to C, and A revokes it
- **THEN** both B and C read end of file on it and their writes fail with `EPIPE`

### Requirement: Exposure ceilings
Each cell SHALL have an exposure ceiling, an egress policy at least its
own, set by its creator and only narrowed. The kernel SHALL refuse
(`EACCES`) a grant, transfer, accepted channel, creation or move after
which information could flow from a cell, through cells
that are not its ancestors, to a cell whose egress exceeds that ceiling.

#### Scenario: No proxy to the internet
- **WHEN** a parent with egress `full` grants a channel between its child with `none` and its child with `full`
- **THEN** the grant is refused, and is accepted once the parent raises the first child's ceiling to `full`

### Requirement: Channels
A channel SHALL join two endpoints and carry messages of at most 64 KiB
and 16 handles, copied by the kernel, with at most 64 messages and
256 KiB queued per endpoint, room its creator reserves from its memory
limit. No memory SHALL be shared through a channel, and a full queue
SHALL block or fail with `EAGAIN`, never with a memory error.

#### Scenario: Bounded queue
- **WHEN** a cell sends 64 messages on a non-blocking endpoint whose peer does not read, then one more
- **THEN** the 65th fails with `EAGAIN`, and once the peer reads one the next send succeeds

### Requirement: Channels as descriptors
An endpoint SHALL behave as a connected `AF_UNIX` `SOCK_SEQPACKET` socket:
each write sends one message, each read takes one, and `poll`, `epoll`,
non-blocking mode, `sendmsg`/`recvmsg` and end of file work as on Linux.
`connect` to, or `open` of, `/run/rhizo/chan/<label>` SHALL give a
descriptor of it.

#### Scenario: Unmodified Python
- **WHEN** a CPython cell connects a `SOCK_SEQPACKET` socket to `/run/rhizo/chan/peer`, waits on it with `selectors`, and exchanges messages with another cell
- **THEN** each message arrives whole and in order, and closing one end gives the other end of file

### Requirement: Opening a channel to a cell
A cell holding the message right on another SHALL open a new channel to
it by connecting to `/run/rhizo/cell/<label>`; the other end SHALL wait on
the target's `/run/rhizo/inbox`, which behaves as a listening
`SOCK_SEQPACKET` socket.

#### Scenario: Accepting from the inbox
- **WHEN** a cell with the message right connects to its handle on a server cell, which accepts on its inbox
- **THEN** both ends exchange messages, and a cell without the right gets `EACCES`

### Requirement: Kernel endpoint
A cell given `/run/rhizo/kernel` SHALL be able to send it requests in the
control protocol's frames to create a child, make channels, derive and
revoke handles, and narrow, re-parent or promote its descendants, each
needing the matching right of the endpoint; a malformed request SHALL be
refused without harm.

#### Scenario: Missing right
- **WHEN** a cell whose kernel endpoint lacks the spawn right asks for a child
- **THEN** the request is refused and no cell is made

### Requirement: Children made by a cell
A child made at a cell's request SHALL be its child: a fork of it, a copy
of a template it holds a control handle on, or a program from its own
files; with limits carved from its budget, egress within its own, a
loopback network of its own, and only the handles the request passes. The creator SHALL receive a control handle on it.

#### Scenario: Child spawned from inside
- **WHEN** a cell with the spawn right forks itself into a child with 16 MiB and egress `none`, passing one channel
- **THEN** the child runs with those limits and that egress, holds only that channel, and the parent holds a control handle on it

### Requirement: Limits of the hierarchy
The kernel SHALL bound: tree depth to 16 below the host, children of one
cell to 1024, handles per cell to 256, derivation chains of a grant to 16,
channels a cell holds to 128, channels waiting in an inbox to 64, rules in
an egress policy to 64, entries in the host's deny floor to 64, and labels
to 64 bytes. A request past a bound SHALL fail with its reason (`EMLINK`
for depth and chains, `EMFILE` for handles and channels, `EINVAL` for
sizes) and change nothing.

#### Scenario: Too deep
- **WHEN** cells make children down to depth 16 and the deepest asks for one more
- **THEN** it is refused with `EMLINK`, and every cell of the chain keeps running

### Requirement: Failures leave a consistent tree
A refused or failed tree operation SHALL change nothing: no cell made in
part, no reservation taken, no handle granted. If the host's link is lost,
cells SHALL keep running, requests waiting for the host SHALL be refused
at their deadline, and audit records SHALL be kept up to the kernel's
bound and the rest counted as dropped.

#### Scenario: Fork that cannot finish
- **WHEN** a fork into four copies runs out of memory after making two
- **THEN** the two are undone, the parent's free budget is what it was, and the request reports the memory error
