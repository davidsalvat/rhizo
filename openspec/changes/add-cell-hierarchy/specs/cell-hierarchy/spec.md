# Spec Delta

## ADDED Requirements

### Requirement: Ownership tree
Every cell SHALL have exactly one parent: the host, or the cell at whose
request it was created. A cell the host creates SHALL be a child of the
host or of the cell the host names. The children of the host are roots.
Suspending a cell SHALL NOT suspend its children.

#### Scenario: Children of a cell
- **WHEN** the host runs cell A, then runs B with parent A, then suspends A
- **THEN** the tree lists A under the host and B under A, A's domain holds both, and B keeps running

### Requirement: Domains are tenants
A domain SHALL be a tenant: a root the host makes or loads and its
descendants, joined by the copies of its cells, which SHALL stay in their
source's domain (roots among them) unless the host places them
elsewhere. A promoted cell SHALL start a domain of its own.

#### Scenario: Copy of a root
- **WHEN** the host forks root A, then A ends
- **THEN** the copy is a root in A's domain with another number, and the domain lives on with it

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
- **THEN** it finds only an empty `inbox` and `generation`, every connection is refused, and only its own descriptors open

### Requirement: Handles and rights
A handle SHALL name one kernel object with rights: on a cell observe,
input, message and control; on a channel endpoint send, receive and
carry; on a view read and write; on an accelerator infer, compute and
stats; on the kernel endpoint spawn, channel, derive, revoke and
narrow. A cell SHALL use handles only through
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
a revoked view SHALL disappear. Dropping a handle SHALL NOT revoke the
handles derived from it: they stay revocable by whoever could revoke them
before.

#### Scenario: Revoked while passed on
- **WHEN** A grants B a channel, B passes it to C, and A revokes it
- **THEN** both B and C read end of file on it and their writes fail with `EPIPE`

#### Scenario: Passed on, then closed
- **WHEN** A grants B a channel, B passes it to C and closes its own, and A revokes it
- **THEN** C keeps using the channel until A's revocation, then reads end of file on it

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

### Requirement: Content sharing stays in its domain
The kernel SHALL share a frame by content (page cache, file pages,
frames a snapshot refers to, and any later cache keyed by content) only
between cells of one domain, with the domain's identifier in the key.
It SHALL never merge frames by scanning memory. A cell that leaves its
domain SHALL first take private copies of the pages it shares with it,
public objects' pages excepted, or the move SHALL fail with `ENOMEM`.

#### Scenario: Same file in two domains
- **WHEN** cells of two domains map the same page of a private file copied into both, and a cell of the first domain maps it again
- **THEN** each domain holds its own frame for it, and the first domain's second map shares its first frame

#### Scenario: Promoted cell
- **WHEN** a cell forked inside a domain is promoted to root and its former sibling writes every page they shared
- **THEN** the promoted cell already held private copies, and its writes to those pages copy nothing

### Requirement: Public objects
Across domains the kernel SHALL share only public objects: executables
and libraries of the host's file sets, read-only template and index
files, and templates the host published. They SHALL be immutable, and
the pages of their mapped files SHALL be loaded when published and kept
until withdrawn and unmapped, so a cell's map of one never depends on
another domain. A write SHALL make the writer's file private.

#### Scenario: Library in two domains
- **WHEN** cells of two domains map the same published library, and one of them maps it again after the other ended
- **THEN** both map one frame per page, and every map is a hit

### Requirement: Published pages give way to cells
Short of memory, the published pages no cell maps SHALL give way, as
executables kept loaded do. A domain that maps one later SHALL keep its
own copy, never one filled for every domain on demand.

#### Scenario: Short of memory
- **WHEN** a cell needs frames that only published pages no cell maps hold
- **THEN** those pages give way and the cell gets its frames, while the published pages a cell maps stay

### Requirement: Copy generation
Each cell SHALL have a 64-bit copy generation, readable in
`/run/rhizo/generation` (8 bytes, little endian), that changes in every
copy of a fork or a template, every load of a snapshot file and every
resume, and that nothing else changes; the source of a fork SHALL keep
its value.

#### Scenario: Fork
- **WHEN** a cell reads its generation, is forked into two copies, and each reads it
- **THEN** the source reads the same value as before, and each copy reads a value unlike the source's

### Requirement: Copies draw different random numbers
The CPython and Node.js templates the project ships SHALL reseed their
user-space generators when their copy generation changed since warm-up,
from the copy's own `getrandom`, so that no two copies of one template,
and no two loads of one snapshot file, draw the same sequence.

#### Scenario: Two children of one template
- **WHEN** two cells are copied from a CPython template frozen after `import random`, and each prints `random.random()` and `os.urandom(16)`
- **THEN** the two copies print different values for both

#### Scenario: Node.js template
- **WHEN** two cells are copied from a Node.js template frozen after its warm-up, and each prints `Math.random()` and `crypto.randomUUID()`
- **THEN** the two copies print different values for both

### Requirement: Published templates hold no secrets
The host SHALL be able to publish a template so that copies of it placed
in other domains share its frames, and only a published template SHALL
share frames with copies in more than one domain. The kernel SHALL refuse to publish a template of a
cell that connected through the network card or received a message, a
handle or a view, unless the host attests it holds no secret; the
attestation SHALL be audited.

#### Scenario: Tainted template
- **WHEN** the host publishes a template of a cell that connected to the host's echo server, then the same with an attestation
- **THEN** the first is refused and the second is accepted with an audit record naming the attestation

#### Scenario: Unpublished template in another domain
- **WHEN** the host places a copy of an unpublished template under a cell of another domain
- **THEN** the copy is a full copy that shares no frame with the template, public objects excepted

### Requirement: Accounting charges the using cell
Memory, file data, kernel records and CPU time SHALL be charged only to
the cell that takes them, never to each ancestor: a subtree SHALL be
bounded by the limits reserved from its parent, and socket buffers SHALL
be charged at their full size when the socket is made.

#### Scenario: Deep tree
- **WHEN** a cell at depth 15 and a root each take memory page by page until refused
- **THEN** each is refused at its own limit, and the deep cell's ancestors' free budgets are unchanged

### Requirement: CPU weights between domains
Each cell SHALL have a CPU weight from 1 to 10,000 (default 100). Busy
domains SHALL share each CPU in proportion to their roots' weights
whatever their numbers of cells, and cells of a domain by their weights
flattened along the tree; an idle part's share SHALL go to the busy
ones.

#### Scenario: Many cells do not buy CPU
- **WHEN** on one CPU a domain of 16 spinning cells and a domain of one spinning cell, with equal weights, run for 2 s
- **THEN** each domain gets half the CPU within 5 %

### Requirement: CPU caps per subtree
A cell MAY have a CPU cap, a rate (such as 1.5 CPUs) bounding its whole
subtree over each period of 100 ms. A child's cap SHALL be at most its
parent's, caps SHALL only narrow, and a subtree that used its cap SHALL
not run until the next period.

#### Scenario: Capped subtree
- **WHEN** a subtree capped at 0.25 CPU spins next to an idle CPU
- **THEN** it gets at most 25 % of the CPU plus one time slice per period

### Requirement: Accelerators are capabilities
A cell SHALL reach an accelerator only through a handle with the rights
`infer`, `compute` or `stats`, narrowed only by derivation to fewer
rights, fewer models or a smaller budget, with device memory, KV bytes,
device time and sessions as budgets carved from the parent's. No handle
SHALL name a device or partition, a non-isolated device SHALL serve one
domain at a time, and a copy SHALL not inherit the handle.

#### Scenario: Asking for more
- **WHEN** a cell holding an accelerator handle with `infer` on one model derives one with `compute`, and one listing another model
- **THEN** both derivations fail with `EACCES`, and a cell with no grant finds no accelerator under `/run/rhizo`

#### Scenario: Copy of a cell with a handle
- **WHEN** a cell holding an accelerator handle is forked by its parent
- **THEN** the copy holds no accelerator handle, and opening a new session in it fails

### Requirement: Accelerator budgets carved by derivation
An accelerator handle SHALL carry its budgets and the models it lists, at
most 64. A derived handle's budgets SHALL be carved from what its source
has free (its budgets less what was derived from it and what its own
sessions use) and come back when it is revoked; a derivation past that
SHALL fail with `EDQUOT` and change nothing. The host's grants of an
accelerator SHALL fit the capacity it registered for it.

#### Scenario: Carving a budget
- **WHEN** a cell holding a handle with 4 sessions derives one with 2 for a child while one of its own sessions is open, then asks for 2 more
- **THEN** the first derivation succeeds, the second fails with `EDQUOT`, and once the child's handle is revoked the same derivation succeeds
