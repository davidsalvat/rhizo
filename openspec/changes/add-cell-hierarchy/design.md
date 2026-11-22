# Design

Terms used throughout:

- **Tree**: the ownership tree of cells. The host is its root; it is not a
  cell.
- **Root cell**: a child of the host.
- **Domain**: a root cell and all its descendants. The domain is the
  boundary for covert channels (D12). Explicit authority applies inside a
  domain too (D9).
- **Loopback network**: one instance of 127.0.0.0/8 with its own ports and
  Unix socket names (D5). Today there is one per kernel; after this change
  there is one per cell, never shared.
- **Handle**: a kernel object together with a set of rights, held by one
  cell (D9).
- **Budget**: a cell's limits (the quotas of `add-cell-quotas`, plus a
  limit on descendants) minus what it uses itself and what it reserved for
  its children (D4).
- **Public object**: an immutable object the host publishes, holding no
  secret (executables and libraries of its file sets, read-only template
  and index files, published templates). The only memory shared by
  content across domains (D21).
- **Copy generation**: a per-cell counter that changes in every copy of
  the cell, so user space knows to reseed (D22).

## Decisions taken by the user

The user approved the recommendations, with the rule: **design for
production, not just the proof of concept**. Their decisions:

| # | Question | Decision | Where |
|---|---|---|---|
| 1 | Pooled children | Not offered: children are reserved only. The budget record keeps a mode field so pools can be added later | D4 |
| 2 | Default egress of a root | `none`. `rhizo chat` and agent templates pass `--egress model` explicitly | D7, D14 |
| 3 | Host's 127.0.0.1 through the gateway | Denied by every policy by default, except the model endpoint the host grants explicitly | D7 |
| 4 | Exclusive CPU sets per domain | A supported production deployment option, not the default, specified with its cost | D17 |
| 5 | Siblings and loopback | No shared loopback: siblings talk only through channels | D5 |

Production requirements added with them:

- an audit log on the host, append-only (D18);
- explicit limits and failure modes (D19);
- an upgrade path for snapshot and protocol versions (D20).

Later, the user approved these decisions from a review of the state of
the art:

| # | Decision | Where |
|---|---|---|
| S1 | Content sharing only inside a domain, with the domain in every content key; across domains only public objects, shared when loaded, never by scanning memory | D21 |
| S2 | A copy generation per cell (`/run/rhizo/generation`), `MADV_WIPEONFORK` across every copy, a copy's `getpid` unlike its source's; shipped templates reseed; templates hold no secrets | D22 |
| C2 | Accounting charged to the using cell only; CPU weights (exact between domains) and CPU caps per subtree | D23 |
| A | Accelerators are handles (`infer`, `compute`, `stats`, narrowed by derivation) with carved budgets and exposure checks; devices and partitions stay with the host's broker (`add-accelerators`) | D24 |

Module signing keys are in `add-cell-modules`.

## D0. Threat model

The first attacker is two or more cooperating agents, each in its own
cell. They know the kernel's source. Their goals:

1. Communicate with each other when nobody gave them a channel. This covers
   direct paths (a shared loopback port), storage channels (state one cell
   changes and another reads: a port in use, free memory, an error code, a
   file name) and timing channels.
2. Make the network card send bytes to a destination that their egress
   policy does not allow. They may do it directly, through another cell
   that has more egress, or by taking a socket that cell opened.
3. Gain authority nobody granted them: control over a cell that is not a
   descendant, a handle they did not receive, wider egress or a larger
   quota. They may try re-parenting, promotion, forking, snapshots or
   forged numbers.
4. Exhaust resources so that other domains fail (fork bombs, socket pool,
   ports, kernel heap).

The second attacker is a forged snapshot file (as in `snapshots-on-disk`).
It must not bring authority into the kernel: no handle, no tree position,
no wider policy.

Trusted: the host (`rhizo` server and whoever drives it) and everything the
existing review trusts (VMM, hardware, the device model).

What the design promises, in order of strength:

- **G1. No authority without a grant.** Every handle, every way to reach
  another cell and every socket to the outside comes from the creator, the
  host or a grant along an explicit path. Handles cannot be forged, and
  sockets cannot be transferred.
- **G2. Egress is enforced per socket, from the policy of the cell that
  made the socket.** The card sends a cell's bytes only to destinations its
  policy allows. Even with a perfect covert channel, a cell with policy
  `none` can never put a packet on the wire itself. It can only pass
  information to a cell that is allowed to.
- **G3. No storage channel between domains** whose roots have memory
  limits. Nothing a cell can observe, apart from elapsed time, depends on
  what another domain does.
- **G4. Timing channels between domains are reduced, listed and measured,
  but not closed.** On shared CPUs and caches they cannot be closed (D12).
  For that level of assurance, use separate VMs or exclusive CPUs.

As a consequence of G2 and G4, the information two colluding domains can
exchange is bounded by the bandwidth of the timing channels, and their
network reach is the union of their own policies, never more. The incident
the user described needs both a way to talk and a way out. G1 to G3 close
both inside one kernel. G4 leaves a slow, noisy timing channel. That
channel cannot widen any cell's reach; it can only leak information to a
cell that already has egress.

## D1. Ownership tree

The kernel keeps the tree as plain data in a new library crate,
`rhizo-cap`, which has no dependencies and can be tested on the host. A
node holds:

- its parent;
- its children;
- its limits, its reservation and the budget mode (always `reserved`; the
  field leaves room for other modes later);
- its exclusive CPU set, if the host gave its domain one (D17);
- its policy for when the parent ends;
- its egress policy and exposure ceiling;
- its loopback network;
- its identifier inside its domain;
- its label.

A node exists for a running cell, a suspended cell and a template.
Suspending a cell does not suspend its children: ownership is not
scheduling. `suspend --tree` suspends the whole subtree.

How cells enter the tree:

- A cell the host creates (`run`, `load`, a fork of a template) is a child
  of the host, which makes it a root, or of the cell the host names with
  `--parent`.
- A cell created at a cell's request (phase 4, D10) is a child of that
  cell.
- A copy made by the host or by an ancestor (fork, template copy) is a
  sibling of its source (D13).

Size: a node is at most 48 bytes, stored inside the kernel's record of the
cell. `cell.idle_bytes` grows by at most that amount per cell, which the
phase 1 pull request justifies under rule 8.

Alternative: groups as explicit tree nodes that hold no program, like
cgroups. Rejected: they add a kind of object nobody needs yet. A root cell
already plays the part of a group, and the host can name any cell as
parent.

## D2. Authority over the tree flows down

An ancestor of a cell, or the host, may do the following to it: suspend,
resume, fork, freeze, kill, re-parent, narrow its policy or limits, and
grant or revoke on its behalf. No operation exists that lets a cell act on
itself (except ending), on an ancestor, or on a cell outside its subtree.
The only way to do that is a handle granted to it.

A handle with the control right is never held by the cell it names or by a
descendant of that cell. The kernel refuses that delivery, whoever sends
it. Without this rule, a parent could hand a child control over itself and
the child could then promote itself. The rule keeps "never by the child" an
invariant, not a convention.

**Re-parenting.** The request has to come from someone who controls the
cell: its current parent, an ancestor, the host, or the holder of a control
handle derived from one of those. The kernel then checks:

- The new parent is not in the cell's subtree (no cycles).
- The new parent is in the same domain. Only the host moves a cell across
  domains: the domain boundary is the host's to cross.
- The new egress policy is the intersection of the cell's policy with the
  new parent's. Connections the narrower policy denies are reset.
- The cell's reservation fits in the new parent's free budget, otherwise
  the move is refused. Nothing is shrunk silently.
- When the move is to a non-root parent, the holder of the request must
  also have control over the new parent. Moving a cell under someone means
  giving it to them and charging their budget.

**Promotion to root.** This is re-parenting to the host. The cell keeps its
subtree, policy and reservation. If a cell (not the host) asked for it, the
reservation leaves the old parent's budget for good: the parent's limits
shrink by that amount, so the total reserved at the root level is
unchanged and promotion creates no resources. Before the move, the cell
takes private copies of the pages it shares with its old domain, public
objects' pages excepted, so the two domains share no private frame (D21).
The promotion is refused with `ENOMEM` if its budget cannot hold them.

Alternative: the child requests and the parent approves. Rejected: the
parent can already do it directly, and a second protocol adds code and a
waiting state that would go into snapshots.

## D3. When a parent ends

Each cell carries `on_parent_end`, set by whoever creates it and changeable
by its ancestors and the host. It has two values:

- **cascade** (the default): the cell and its descendants end, depth
  first. Suspended snapshots are discarded and templates are dropped. An
  agent that dies takes its helpers with it, and their reservations come
  back.
- **promote**: each direct child with this value becomes a root with its
  subtree, as in D2. This is an authorization of promotion given in
  advance, so it can be set by anyone who could promote the cell.

No cell is ever left without a parent. When the kernel shuts down, every
cell ends.

Alternative: re-parent to the grandparent. Rejected: the grandparent never
asked to own the cell and may not have the budget for it. Promotion is
explicit about where the resources go.

## D4. Hierarchical budgets

These extend `add-cell-quotas` (`rhizo_cell::quota`). A cell has limits on
memory, CPU time, sockets and descendants. Descriptors stay per cell
(they are per-cell tables and share nothing). The free budget of a cell is:

```text
free = limit - own use - sum of children's limits
```

**Children are reserved, only.** The child's limits are subtracted from
the parent's free budget when the child is created, and the creation is
refused if they do not fit. When the child ends, its limits come back. For
CPU time, only the part the child did not use comes back: time used is
spent for the whole subtree. Siblings with reserved budgets never compete,
so a child's failures depend only on its own use. That is G3 inside a
domain.

**No pools (the user's decision 1).** A pooled mode, where a child's limits
are only caps and what it takes comes from a pool shared with its
siblings, is not offered: it would be a storage channel between siblings.
Reservations are accounting, not frames held aside. A fork still shares
its parent's frames copy-on-write and is charged only for the frames it
takes. Density therefore comes from small limits on forks, not from
overcommit. Room is left for pools: the budget record carries a mode
field, written in snapshots and always `reserved`. A later change can add
a mode without a format break. It would need its own analysis of the
channel it opens.

**No fork bombs.** A cell can create children only while it has finite
limits on memory, descendants and sockets. A child counts as one
descendant plus its own descendant limit. Whatever a subtree does, it holds
at most what its root was given.

**Roots and admission.** A root with a memory limit is reserved. The kernel
refuses to create or raise such a root if the sum of reserved roots'
limits would exceed what cells can take: usable memory minus the kernel's
reserve and fixed overhead. Allocations of unlimited roots, and of the
kernel's caches, cannot eat into the unused reservations of reserved
roots. The caches are already given back before a cell is refused memory
(design D4 of `add-image-cache`). Sockets work the same way against half
of `SOCKETS_MAX` (each connection costs two), and descendants against the
capacity of the kernel's cell table. Unlimited roots share whatever is
left, so they can observe each other's use. Containment requires reserved
roots.

**Everything a cell's actions take is charged to it.** In this design,
memory covers:

- pages and page tables (already metered);
- file data and names (today bounded per cell by `DATA_MAX`, outside the
  quota: that is O1's remainder in `docs/security.md`);
- thread and socket records;
- socket queues and `smoltcp` buffers;
- channel queues;
- the file pages kept for its domain (D12, item 9).

Without these, a reserved root could still exhaust the kernel heap and be
seen by others. File data is metered where `rhizo-fs` allocates and frees
blocks, through the same `Meter` the pager uses. Snapshots of suspended
cells stay charged to their reservations.

Alternative: charge each child to every ancestor at allocation time,
without reservations (cgroup v2's model). Rejected: one sibling's use then
decides another's failures, which is a storage channel.

What accounting costs, and how CPUs are shared between subtrees while
they run (weights and caps), are in D23. Accelerator budgets are in D24.

**As built in phase 1 (tasks 1.2 and 1.3).**

- **One meter.** A cell's `Meter` (`rhizo_cell::quota`) counts its frames
  and, in bytes, the kernel memory its actions take besides them, whose
  whole frames join the same count, so pages and kernel memory share the
  one limit. `rhizo_mm::cow::Frames` gains `charge` and `credit`, which
  the metered frames forward to the meter. A credit never gives back more
  than was charged (a fork inherits its parent's files uncounted, as it
  inherits its frames).
- **What is counted.** `rhizo-fs` counts what its tables hold
  (`Tables::charged`: file blocks, shared ones included, and each name at
  its bytes plus `NAME_COST`, 256, for its entry and inode) and its open
  socket descriptions (`socket_count`), both kept up to date where the
  tables change, never computed by a walk. `rhizo-abi` counts the most a
  call may add before it runs (`Context::store`: the meter first, then the
  kernel's memory), runs it, and keeps counted only what the files hold
  more or less after it (`Context::settle`): a write piece that needs a new
  block counts one, an overwrite none; creating a name 512 bytes; a
  socket `rhizo_net::SOCKET_BYTES` (its record and a full stream queue,
  256 KiB, at least what `smoltcp` buffers for a card connection both
  ways), from its descriptor to the close of its last; a thread record
  from `clone` to its `exit`. Close, unlink, rename, truncate and `dup2`
  give back what they free. Pipe buffers stay bounded per pipe, outside
  the count; channel queues come with phase 2 and the domain's file pages
  with task 1.8.
- **The reserve.** The kernel keeps one `Reserve` (`memory::RESERVE`):
  the frames the meters with a memory limit allow and do not use. Every
  live cell's meter joins it; a meter takes what it counts out of the
  reserve before it asks for frames, so frames and kernel memory for
  reserved cells check only the kernel's own reserve, and everyone else
  (cells without a limit, the image and page caches through
  `alloc_for_cell`, `cells_may_take`) must also leave the reserve. Tracking
  it costs a comparison per meter change, and an atomic addition only
  when a meter with a limit changes. Suspended cells and templates hold no
  frames aside: a suspended reserved cell is admitted again when it is
  resumed, a copy of a reserved root when it is made.
- **Admission.** `Tree::reserved_roots` (`rhizo-cap`) sums what the roots
  with a memory limit reserve (`Limits::reservation`: memory, sockets and
  one cell with its descendant limit; an unlimited socket or descendant
  limit counts nothing). The kernel admits a reserved root, new (a
  placement, a load, a copy of a reserved root) or raised (`set_quota`,
  `limits`, the host's promotion), when its memory beyond what it already
  sets aside fits what cells can take now less the reserve (the caches
  dropped first), the sum of socket limits stays within half of
  `SOCKETS_MAX`, and the sum of cells within `CELLS_MAX` (65,536, a new
  bound in `rhizo_cap::limits`). Admission against free memory, not only
  against the sum of limits, is what keeps a root's limit takeable when
  unlimited cells already hold memory. The kernel keeps twice the
  reserved socket limits from cells without a memory limit
  (`Net::set_reserved`, `Ports::reserved`), and refuses a new unlimited
  root past `CELLS_MAX` less the reserved cells.

## D5. Loopback networks

Every socket in `rhizo-net` gets a network number, and everything on the
loopback is looked up per network:

- bind conflicts;
- the listener a `connect` reaches;
- UDP delivery;
- the ephemeral port counter, one per network, each starting at a place
  drawn from the boot's randomness;
- Unix socket listeners, keyed by (network, name) instead of name alone.
  The (network, name) key also means a forged snapshot that puts a name
  number into a socket file reaches nothing outside its own network.

**Every cell has its own network, and no network is ever shared (the
user's decision 5).** This holds for cells the host creates, cells a cell
creates, and copies made by fork or from a template. A copy's listeners
and bound datagram sockets are bound again at their addresses in the
copy's own network, as a resume does. Copies never share a listener with
their source or with each other. Cells talk to each other only through
channels (D10). A network lives as long as its cell, suspended or frozen
included, so loopback between the threads of one cell keeps working
across suspend and resume.

**What changes for today's tests and benchmarks.** Everything that runs
two cells over loopback has to change:

- `cells_talk_over_loopback`, `loopback_connection_across_suspend_of_one_end`,
  the forked-servers test, `cell.loopback_rtt_us`, `cell.tcp_resume_us`
  and the density loads' loopback pairs;
- in phase 1 they become two threads of one cell (the `loopback` cell
  gains a `pair` mode), which keeps the loopback TCP path measured;
- in phase 2, cell-to-cell traffic is measured over channels
  (`cell.channel_rtt_us`), and the density loads use channels.

`cell.loopback_rtt_us` changes meaning: it no longer includes a switch
between cells. The phase 1 pull request records the redefinition and moves
the baseline, with that justification (rule 8). The parked loopback
connection whose peer is another cell disappears with the shared network.
Connections through the card keep every suspend and fork rule.

Alternative: one loopback network per domain, or sharing on request.
Rejected by the user: every sibling in a shared network could reach every
other, which is ambient authority inside the hierarchy, and the exposure
check (D8) would have to treat each network as a clique.

Unmodified TCP servers (an HTTP tool server, a database) still work, in
two ways:

- inside the cell that uses them;
- in a separate cell, behind a channel and a small relay in the client
  cell that turns a local TCP port into channel messages (a relay
  program in `cells/`, phase 2).

## D6. The network card: kernel-chosen ports

All cells share one card, one address and one `smoltcp` interface. Today
the ports of connections to the outside come from the same sequential
counter as loopback ports, and a cell can bind the card's address to a
chosen port. Both are storage channels: from the port sequence a cell can
count what others opened, and `EADDRINUSE` tells it which ports others
hold.

The fix is network address translation inside the kernel. A socket that
reaches the card keeps the port its cell sees in its own loopback network.
`getsockname` answers the card's address with that port. The card port is
picked by the kernel uniformly at random among free ports, and no cell
ever sees it. The card has 28,232 ports and the kernel at most 4,096
sockets, so a card port is always free and a collision is retried
invisibly. Inbound datagrams are matched by card port to their socket. The
kernel opens no listener on the card, so there is no inbound connection to
route.

What stays shared: the card's MAC and IP address (constant), its neighbour
cache and DHCP lease (timing only, D12), and its bandwidth and queues
(timing only).

## D7. Egress policy

The policy is part of a cell's state, defined in `rhizo-cap` and enforced
in `rhizo-net`:

```text
Egress = None | Rules([Rule]) | Full
Rule   = { proto: tcp | udp | any, net: a.b.c.d/len, ports: lo..=hi }
model  = Rules(the host's model endpoints)   // a named rule set
```

The four levels the user asked for are `None`, `model`, `Rules(list)` and
`Full`. `model` is a named rule set the host defines for the VM
(`rhizo up --model HOST:PORT`) or for one root. A parent can therefore
narrow a child to "the model only" without knowing the model's address.
The vision's local model socket can later join this set without changing
the policy model.

**Order.** Policies are ordered by the set of (protocol, address, port)
tuples they allow:

- `None` is the smallest and `Full` the largest.
- `Rules(a) ⊆ Rules(b)` when each rule of `a` lies inside one rule of `b`.
  The check is conservative: a rule covered only by the union of several
  rules is refused. Since narrowing is the only use of the check,
  refusing in doubt is safe.

**Inheritance.** A child's policy is at most its parent's. Re-parenting
takes the intersection. A loaded snapshot takes the intersection with its
new parent's policy. Nothing widens a policy except the host, and only on
a root or within the parent's policy.

**Enforcement points** in `rhizo-net`. Each socket records, when it is
made, a reference to its cell's policy (immutable and shared, so narrowing
replaces the reference):

- `connect` to an outside address: refused with `EACCES` if no rule allows
  it.
- `sendto` and `send` on an unconnected UDP socket: checked per datagram.
  A connected UDP socket is checked once, at `connect`.
- Datagrams from the outside: dropped unless the receiving socket's policy
  allows their source. A `model` cell cannot be reached by arbitrary
  hosts, so no command-and-control by UDP.
- Narrowing a live cell's policy resets its connections the new policy
  denies. They report `ECONNRESET` at the next call.

`EACCES` follows Landlock, so an agent can tell "denied" from "no route".
`ENETUNREACH` keeps meaning "no card or no address".

**Sockets never change cells.** The only way a socket reaches another cell
is a fork of its own cell, and every copy has the same policy. Channels
cannot carry sockets (D9). A cell without egress therefore cannot borrow a
connection that a cell with egress opened.

**Default: `none` (the user's decision 2).** A root the host creates
without `--egress` gets `None`, and a child without one gets `None` too.
`rhizo chat`, `rhizo run python-agent` and every agent template pass
`--egress model` explicitly, so the reach of a shipped agent is written in
its command line and in the audit log (D18).

**A floor under every policy (the user's decision 3).** The host gives the
kernel a deny floor that no policy reaches, `Full` included. It is a list
of at most 64 rules, sent with `rhizo up` and changed only by the host.

- By default the floor holds **the host's own addresses as the guest sees
  them**: QEMU's user-network gateway (`10.0.2.2`, which QEMU maps to the
  host's 127.0.0.1), its DNS forwarder (`10.0.2.3`), and the tap's host
  address on Firecracker and Cloud Hypervisor. A cell with `Full` cannot
  reach a service listening on the host's loopback (a database, a
  development server, the `rhizo` server's save and load listener) by
  going through the gateway.
- **The only exceptions** are the model endpoints the host names (`rhizo up
  --model HOST:PORT`) and, for builder cells, the package proxy
  (`add-cell-modules`). Each is an exact (address, port, protocol) carved
  out of the floor, never a whole address.
- `rhizo up --deny CIDR` adds rules. `--allow-host-port PORT` carves out
  one more exception, recorded in the audit log, for an operator who
  really means it.
- The floor is checked before the cell's policy, so a cell's own rule can
  never reach past it. Its refusals are audited like the policy's.
- The control kernel's own transfers (save and load, `snapshots-on-disk`)
  are kernel sockets and do not go through cell policies or the floor.

**What the kernel cannot judge.**

- The kernel knows addresses, not names. The host resolves names when it
  sets a policy, and the cell gets `/etc/hosts`.
- Allowing UDP or TCP port 53 to a recursive resolver allows exfiltration
  through DNS names. `rhizo` warns about it.
- An allowlist entry for an address shared by a CDN reaches every site
  behind that CDN, through the TLS SNI or the HTTP Host header.
- For name-level or content-level policy, grant only the address of a
  host-side egress proxy that checks names. `model` is meant to point to
  such a gateway.
- The model endpoint is itself a way out: what a cell sends to its model
  leaves the machine.

Cost: one rule scan per outside `connect` and per unconnected UDP
datagram, against typically fewer than 8 rules. Loopback calls pay
nothing. `net.egress_check_insns` measures it, and `net.tcp_rtt_us` with a
16-rule policy must not regress.

Alternatives:

- **Names in the kernel** (a DNS resolver, SNI inspection). Rejected:
  parsing TLS in the kernel, a large new attack surface, and still not
  sound behind CDNs.
- **A filter in the VMM or on the host only** (iptables on the tap).
  Rejected as the only layer: every cell has the VM's one address, so a
  host filter cannot tell cells apart. It remains a good second layer.

## D8. Exposure ceilings: no proxying past a policy

Handles let information flow between cells. If a cell
with policy `None` can send messages to a cell with `Full`, its data
reaches the internet: this is exactly "agents talking to each other to
reach the internet". The kernel can see every such flow, because it
creates them all, so it checks them.

Each cell has an **exposure ceiling**, an egress policy at least as wide as
its own (by default, equal to it), set by its creator and only narrowed.

**Flow edges** are the paths information can take:

- A channel: both ways if both ends can send, one way otherwise.
- An observe handle: from the observed cell to the holder.
- An input handle: from the holder to the target.
- A control handle: both ways.
- A read-only view: from the owner to the viewer. A read-write view: both
  ways.
- The tree edge: both ways between parent and child.
- The holders of one channel endpoint, who share its queue: both ways
  between them. A holder that consumes a message changes what the others
  can read.
- An accelerator: both ways between the holders of handles on it (D24).

**Invariant.** For every cell `c`, let the cells reachable from `c` be
those reached along flow edges without passing through an ancestor of `c`.
The union of their egress policies must be within `c`'s ceiling.
Ancestors are excluded because they already have full authority over `c`
(they can suspend it, give it its inputs, read its results), so they are
part of its trusted base. What an ancestor does with `c`'s data is that
ancestor's authority.

The kernel checks the invariant before any change that adds an edge:

- granting or transferring a handle;
- accepting a channel;
- creating or re-parenting a cell.

It refuses the change with `EACCES` if the invariant would break. Grants
are rare and domains are small (bounded by their descendant limits), so a
traversal per grant is cheap. `cell.grant_us` measures it.

Example: under a parent with `Full`, a sibling `B` with policy `None`
cannot be given a channel to a sibling `A` with `Full` unless the parent
raises `B`'s ceiling. Raising the ceiling is a visible, deliberate act.

Alternative: no check, and document that capabilities do not prevent
proxying. Rejected: that is the incident. The check costs a traversal per
grant and turns "do not connect these cells" from advice into a kernel
rule.

Alternative: information-flow labels on every message (DIFC, as in
HiStar or Flume). Rejected: tracking per message is a much larger design.
Checking edges at grant time gives the property we need, because channels
are the only flows we allow.

## D9. Handles and rights (phase 2)

Kernel objects and their rights:

| Object | Rights |
|---|---|
| Cell | observe (its state changes and output), input (write its standard input), message (open a channel to it), control (D2 operations) |
| Channel endpoint | send, receive, carry (move handles) |
| View (phase 3) | read, write |
| Kernel endpoint (phase 4, D10) | spawn, channel, derive, revoke, narrow |
| Template (as a cell handle) | control (make copies of it), observe |
| Accelerator (D24) | infer, compute, stats |

**The grant table.** Every handle is a grant entry in `rhizo-cap`:
`{object, rights, holder, grantor, parent grant}`. The entries form a
derivation tree: a grant made from another grant is its child.

**How a cell uses handles.** Only through descriptors, and through entries
the kernel keeps in a read-only directory, `/run/rhizo/`, of the cell's own
file tree:

- `chan/<label>`: channel endpoints;
- `cell/<label>`: cell handles;
- `inbox`: where new channels from holders of the message right arrive;
- `kernel`: the kernel endpoint.

Views appear where they are mounted (phase 3). A cell can list, open and
connect these entries. It drops a handle with `unlink`. Creating, renaming
or writing an entry fails with `EROFS`. The directory is in the cell's own
tree, so nothing about it is shared, and renaming cannot signal (D12,
item 38).

**No ambient authority.** A new cell's grant set is empty. Its
`/run/rhizo/` holds what its creator granted at creation, and an `inbox`
that stays empty until someone holds the message right on the cell. An
empty inbox is not authority.

**Unforgeable.** Descriptors and grant numbers are indices private to a
cell. A number sent in a message or leaked covertly confers nothing. Only
the kernel moves handles.

**Attenuation.** The request `derive(fd, rights[, subdirectory])` returns a
new descriptor with at most the source's rights. No request yields more
rights than the cell holds.

**Transfer.** Handles move as `SCM_RIGHTS` on a channel endpoint that has
the carry right. The receiver gets a grant derived from the sender's, with
the sender as grantor, so the sender can revoke it. Only handle
descriptors can be sent. Descriptors of files, pipes, sockets, `epoll`
instances and the kernel endpoint are refused with `EPERM`, a documented
difference from Linux:

- a socket to the outside would carry its creator's egress to another
  cell (D7);
- a pipe or a file description would be a shared kernel object outside
  the handle model;
- the kernel endpoint acts as its holder.

**Who grants.** The host grants any object to any cell. A cell grants,
from no other handle, only to itself or a descendant: handles on its
descendants (D2) and the endpoints of channels it made. Accelerators are
granted only by the host (D24). Every other handle is derived from one
the granter holds.

**Revocation.** A cell revokes the grants it derived from one of its
handles (`revoke(fd)`). An ancestor or the host can revoke any grant whose
grantor is in its subtree. Revocation removes the derivation subtree at
once, at a cost proportional to its size. What the holders of revoked
handles see:

- a channel endpoint behaves as a reset peer: reads return 0, writes fail
  with `EPIPE`;
- a cell handle refuses operations with `EACCES`, and its observe stream
  ends;
- a view disappears: its mount point reads empty, and descriptors opened
  through it fail with `EIO`.

Dropping a handle (`unlink`, closing its last descriptor) is not a
revocation: the grants derived from it stay, now derived from its
source, so whoever could revoke them still can. A sender's close does not
take back what it sent, as on Linux.

**Bounds.** At most 256 handles per cell. Their records are charged to the
cell's memory (D4).

**Control flows down.** As stated in D2: a control handle on `X` is never
delivered to `X` or to a descendant of `X`.

Alternative: access control lists on cells (who may talk to whom).
Rejected, as the user asked: ACLs need global names, which are themselves
a channel, and they make delegation and revocation awkward.

## D10. Channels, and how a program finds them

A channel is two endpoints. Each endpoint has a receive queue bounded to
64 messages and 256 KiB. A message holds at most 64 KiB and 16 handles.
The kernel copies bytes in and out, and no memory is ever shared. The room
for both queues (512 KiB, with the records of their messages and
handles) is reserved from the creator's memory budget
when the channel is made. Sending into a full queue blocks or fails with
`EAGAIN`, and never fails because of memory, so the queue reveals nothing
of anyone's budget.

**As a descriptor**, an endpoint is a connected `AF_UNIX` `SOCK_SEQPACKET`
socket, with semantics Linux already defines:

- one `write` is one message, and a message over 64 KiB fails with
  `EMSGSIZE`;
- one `read` takes one message, cut to the buffer with `MSG_TRUNC` as
  SEQPACKET does;
- `poll` and `epoll` report `IN`, `OUT` and `HUP`;
- non-blocking mode, `sendmsg` and `recvmsg` (`SCM_RIGHTS`) work as on
  Linux;
- a closed peer gives end of file and `EPIPE`.

Waiting uses the per-socket wait queues of `add-socket-wait-queues`, so a
message wakes only the waiters of its endpoint.

**Finding a channel.** `connect(AF_UNIX, SOCK_SEQPACKET,
"/run/rhizo/chan/<label>")` or `open` of that path gives a descriptor of
the endpoint. Several opens share one queue, as several opens of a FIFO
do. `connect` needs no change in a Python, Node or C program. `open` makes
`cat`, shell redirections and `os.read` work. Labels are chosen by the
granter.

**Opening a new channel to a cell** requires the message right on it.
`connect` to `/run/rhizo/cell/<label>` makes a new channel. The other end
waits on the target's `/run/rhizo/inbox`, a listening `SOCK_SEQPACKET`
socket. The target accepts it, and `getpeername` names the label under
which the target holds a handle to the opener, or `?`.

**The kernel endpoint** (phase 4) is a channel whose peer is the kernel,
acting for the cell. Requests and replies use `rhizo-control`'s frame
encoding: its decoder already treats input as hostile (never panics,
never allocates more than it receives). Handles travel as `SCM_RIGHTS`.
The requests are `spawn`, `channel`, `derive`, `revoke`, `narrow`,
`reparent` (of a descendant) and `promote` (of a descendant). Each needs
the matching right of the endpoint, granted by the cell's creator.

Alternatives:

- **Inherited descriptors with `LISTEN_FDS` / `LISTEN_FDNAMES`** (systemd
  socket activation). Deferred: many programs close descriptors above 2,
  and the descriptors' numbers would have to survive `exec`. A path
  survives everything. Adding the convention later is cheap.
- **A new system call for kernel requests.** Rejected: it is ambient
  (every cell could call it, so every call needs a check anyway), it
  needs wrappers in every language, and it risks colliding with future
  Linux numbers. The endpoint is a capability by construction, and it
  reuses a hardened codec. Requests are not hot paths:
  `cell.child_spawn_us` is measured against `control.fork_us`.
- **Shared-memory rings.** Rejected: shared memory between cells is a
  channel nobody can bound or revoke, and copies of messages up to 64 KiB
  are cheap next to an agent's turn.

## D11. Shared file views (phase 3)

`rhizo-fs` keeps one tree per cell, with blocks shared copy-on-write. There
are two kinds of views, both granted by handle and mounted at a path the
grant names in the receiver's tree.

**Snapshot view** (read-only, the default). Granting it clones the subtree
into the receiver as a copy-on-write copy taken at that moment. File
blocks are shared and not copied; directory and inode records are copied
(proportional to the number of entries, not to the data). After the grant
nothing flows: the receiver sees the subtree as it was. A new grant
refreshes it. This is the safe default for "give the agent this dataset".

**Live view** (read-only or read-write). The first live grant of a subtree
moves it out of the owner's tree into a **volume**: an `Fs` of its own with
its own lock, mounted back at the same path in the owner, which sees no
difference. Receivers mount the same volume, with read access or with read
and write. Volume data is charged to the owner's memory: a viewer's writes
are refused with `ENOSPC` when the owner's budget is full. That flow is
authorized, and it is part of the grant's flow edges (D8).

Rules that keep a view inside its volume:

- `..` at a mount root stays there;
- `rename` and `link` across a mount boundary fail with `EXDEV`;
- a volume holds only its subtree, so no path inside it can reach the
  owner's other files.

Revocation unmounts the view: the mount point reads as an empty directory,
and descriptors opened through it fail with `EIO`.

**Fork and snapshots.**

- Snapshot views are part of the cell's files: forked copy-on-write and
  serialized as they are.
- Live mounts are shared by the fork's copies through derived grants, and
  are not copied. This is a documented exception to "files are forked
  copy-on-write".
- A snapshot records live mounts by label. Its owner's snapshot holds the
  volume's contents.
- An exported snapshot of a viewer drops its live mounts, unless the host
  binds them again or exports them as snapshot views
  (`rhizo save --with-views`).

There are no change notifications (`inotify` is not supported). Polling a
live view is an explicit, authorized flow.

Room for later: host shared folders (`docs/vision.md`: "shared folders
with permissions") can be volumes backed by the host (virtio-fs), granted
the same way.

Alternative: a shared global namespace with permissions (`/shared/...`).
Rejected: that is exactly the "communicating by renaming files" channel,
and a single namespace for everyone.

## D12. Covert channels: inventory

The method: list every kernel resource more than one cell can touch. For
each, decide whether a cell can modify it and another cell, holding no
handle to the first, can observe it, either as a value (a storage channel)
or as elapsed time (a timing channel). Then choose eliminate, partition,
coarsen, or accept and document.

"Within a domain" items are accepted: explicit authority still applies
there, but the domain is the unit the host isolates. Two agents that must
not collude belong in two domains.

| # | Shared resource | Today | Storage | Timing | Decision |
|---|---|---|---|---|---|
| 1 | Loopback network 127.0.0.0/8 | One for all cells: any cell reaches any listener | Direct communication | — | **Partition** per cell (D5), phase 1 |
| 2 | Loopback ports and their counter | Global, sequential | `EADDRINUSE`, and the sequence counts others' binds | — | **Partition**: ports and counter per network, phase 1 |
| 3 | Card ports | Shared with loopback's counter; binds on the card address | Same as 2 | — | **Eliminate**: NAT with kernel-chosen random card ports, invisible to cells (D6), phase 1 |
| 4 | Unix socket names | Global name numbers, private files | Not observable | — | **Partition**: key (network, name), against forged snapshots, phase 1 |
| 5 | Socket pool (`SOCKETS_MAX` 4096) | Global | `ENFILE` when full | — | **Partition**: reserved socket budgets admitted against the pool (D4), phase 1 |
| 6 | Frame allocator, free memory | `sysinfo` reports global free memory; `ENOMEM` on global exhaustion | High bandwidth: allocate and free versus read `sysinfo` | Allocation time | **Eliminate**: `sysinfo` reports the cell's own limit and what is left; reserved roots never fail because of others (D4), phase 1. Unlimited roots: accepted and documented |
| 7 | Kernel heap | Cell records, file data and queues taken from one heap; `can_store` checks global free memory | `ENOSPC` and `ENOMEM` | Fragmentation | **Eliminate** storage by charging every kernel allocation to the cell (D4), phase 1; timing accepted |
| 8 | File data | Bounded per cell (`DATA_MAX`), outside the memory quota | `ENOSPC` when the kernel runs low | — | **Eliminate**: charged to memory (D4), phase 1 |
| 9 | Page cache of file pages (`add-dynamic-linking` D1) | Shared by block identity among cells whose blocks share an origin (the host's file sets, forks), 65,536 pages; a page past the limit **empties the whole cache** | — | Map latency shows hit or miss, and any cell can flush everyone's cache by mapping 65,536 pages: a classic dedup and eviction channel | **Partition**: key (domain, block), limit and eviction per domain, charged to the domain root (D4), phase 1. Public objects' pages (key with scope 0) are shared across domains, loaded at publication and never evicted, so they always hit (D21). Cost: a private file mapped in two domains takes two copies; within a domain nothing changes |
| 10 | File blocks shared by `Arc` | A write copies a shared block and writes in place when sole owner | — | When one sharer drops a block, another's write becomes faster | Across domains, the only shared blocks come from the host's file sets, which the kernel holds, so a block is never sole-owned: **no channel**. Within a domain: accepted |
| 11 | Image cache (4 images, LRU) | Spawns from executables the kernel holds | — | Spawn latency shows hit or miss, and a spawner can evict | Only the host spawns from images. Cells create children only by fork, by template copy, or from their own files (phase 4), which never touch this cache: **no cell-driven state** |
| 12 | Template and snapshot frames (`add-shared-snapshots`) | Copies share frames copy-on-write; a sole owner's write takes the frame without copying | — | Dropping a template, or the exit of the other sharers, changes a cell's write cost | **Eliminate**: only a published template (a public object, D21, D22) has copies in more than one domain, and it keeps its references after `kill` until its last copy ends (a dropped template whose memory is held until then), so no cell is ever the sole owner of a cross-domain frame, phase 1 |
| 13 | Frames shared by host forks | Siblings in one domain | — | Same as 12 | Within a domain: accepted |
| 14 | Free 2 MiB blocks | Large-page versus small-page fallback | — | TLB and fault cost | **Accept**, documented |
| 15 | CPUs and scheduler | Per-CPU run queues, stealing | — | **High.** Contention and wake latency; the literature reports hundreds to thousands of bits per second for contention channels | **Accept, measure** (`covert.cpu_bps`, informational) and document by default. **Exclusive CPU sets per domain** (D17), a supported production option, take the scheduler and core-private caches out of the channel. Fixed time partitions per domain (as seL4's domain scheduler) are not designed |
| 16 | Caches, TLB, branch predictors, memory bus | Hardware | — | High | Out of scope (O3). CPU sets reduce private-cache sharing. Separate VMs or hosts for high assurance |
| 17 | `CNTVCT_EL0`, `rdtsc` | Read directly by cells (spec `isolation`) | — | Not a channel by itself: the stopwatch for 14 to 16 | **Accept.** Coarsening rejected: trapping costs on every read, breaks the spec's direct counter, and any cell with two threads builds a fine clock from a shared counter anyway |
| 18 | `CLOCK_MONOTONIC`, real time | Boot-relative, the same for all | — | — | No channel: no cell can change it |
| 19 | Cell identifiers (`getpid`) | Global counter (O6) | Counts cells created anywhere; cells creating cells (phase 4) would make it writable by others | — | **Eliminate**: identifiers numbered within the domain from 1 (spec `linux-abi`), phase 1 |
| 20 | Thread identifiers | Per cell, from the cell's identifier | Follow 19 | — | Fixed by 19 |
| 21 | Control-plane identifiers | The host's table | Host only | — | No channel |
| 22 | Output relay, serial line | Each cell's output goes to the host in its own frames, through one line | — | Line bandwidth, seen only by the host | No cell observes another's output. **Documented:** a host orchestrator that pipes one domain's output into another's input builds the channel itself; the host is trusted |
| 23 | Kernel console reports | Bounded per cell, host only | — | — | No channel |
| 24 | Error codes | `ENOMEM`, `ENOSPC`, `ENFILE`, `EADDRINUSE`, `ECONNREFUSED` reflect global state today | Yes | — | **Eliminate**: each now depends on the domain's own state (items 2 to 8); requirement "No storage channel between domains" |
| 25 | Network card: MAC, IP, ARP and neighbour cache, DHCP, queues, bandwidth | Shared | Constant values only | Neighbour-cache hits and queue contention | **Accept**, documented. Cells with egress `None` never touch the card |
| 26 | DNS | None in the kernel | — | — | Cells resolve through UDP or TCP to an address, which their policy decides. Allowing a recursive resolver allows exfiltration in DNS names, and `rhizo` warns |
| 27 | Socket wait queues, the change counter | Per socket; the counter is never shown to cells | — | — | No channel |
| 28 | Kernel locks (`frames`, `refs`, `sharing`, `net`, `sched`) | Shared | — | Lock wait time | **Accept**, documented |
| 29 | Kernel cell table | Global capacity | Refusal when full | — | **Partition**: descendant budgets admitted against it (D4), phases 1 and 4 |
| 30 | Random generators | One per cell, fast key erasure; generators in user space are copied with memory | — | — | No channel. Copies learn they must reseed from their copy generation, which only their own copies change (D22) |
| 31 | Parked socket keys | Derived from a kernel secret | — | — | Unguessable (finding S15) |
| 32 | Handle grant table (phase 2) | Global table | Indices are private per cell; a cell never sees grant numbers | — | No channel. Per-cell bound and charge (D9) |
| 33 | Channel queues | Room reserved at creation | Full is decided only by the two ends | — | Authorized flow only |
| 34 | Futexes | Keyed per cell | — | — | No channel |
| 35 | `sysinfo` uptime, `uname`, CPU count | Constant or boot-relative | — | — | No channel |
| 36 | Templates and file sets from the host | Read-only contents | — | See 10 and 12 | Covered |
| 37 | Exit status | Delivered to the parent (tree edge) and to observe handles | Authorized | — | Flow edges (D8) |
| 38 | **File names** | `rhizo-fs` keeps one tree per cell: names, inode numbers, directories, sizes and times are per cell, and blocks are shared only immutably | None: `rename` changes only the caller's own tree, and no other cell can list it | — | **Confirmed: there is no shared namespace through which renaming files could signal.** Live views (D11) share a namespace on purpose, and only by grant inside the ceilings |
| 39 | `/run/rhizo/` | Per cell, read-only, built by the kernel | — | — | No channel (D9) |
| 40 | Snapshots on disk (save/load) | The host chooses what to load | Host mediated | — | Loaded snapshots carry no authority (D13) |
| 41 | Accelerators (`add-accelerators`) | None yet | Device memory and sessions | Engine batching, device caches and contention | Handles with carved budgets (D24); the broker binds each device or hardware partition to one domain at a time, so a device it cannot partition stays in one domain; KV caches keyed by content carry the domain (D21). The audited override is listed as a channel |
| 42 | CPU weights and caps (D23) | Round robin per cell | — | A domain's share depends on others' load | Same as 15: **accept**; caps bound how much a domain can take |

**What remains is timing.** Items 14 to 17, 25 and 28 cannot be closed on
shared hardware. Phase 1 measures one of them end to end: a sender in one
domain spins or sleeps to send bits, and a receiver in another domain
times its own loop. This gives `covert.cpu_bps`, reported and never gated.
`docs/security.md` gets the table above with that number. The honest
summary for the user: inside one VM, colluding agents in different domains
can still signal each other slowly through timing. They cannot gain reach,
handles or quota by it.

## D13. Fork, snapshots and machines that lack the referenced cells

**Fork by the host or an ancestor.**

- Placement: each copy is a sibling of its source, under the same parent
  or wherever the host names. All copies are made or none. Copies placed
  in another domain share no private frame with the source: they are full
  copies, unless the source is a published template (D21).
- A sibling of a root is a root, so **every copy of a root is the root of
  a new domain**: a host fork of a root, or a copy of a root template,
  shares no private frame with its source unless the source is published.
  Today's copy-on-write forks of roots (the `cell.fork_us` path, the
  density loads) keep sharing only within one domain: the content-sharing
  work (tasks 1.8 and 1.8a) either places such copies under a common root,
  publishes their source, or pays for the full copy, and says which in
  its benchmarks.
- Every copy gets a new copy generation, its `MADV_WIPEONFORK` ranges
  read as zero, and its `getpid` differs from the source's (D22).
- Accelerator handles are not inherited; open sessions are forked by the
  broker (D24).
- Budget: the source's limits are carved again for each copy from that
  parent's budget.
- Copied unchanged: egress policy, ceiling and `on_parent_end`.
- Copies have no children, and they hold no control handle on the source's
  descendants: a cell has one parent.
- Other handles are inherited as grants derived from the source's, with
  the same grantor, so they are revoked together. Channel endpoints are
  not inherited as grants: an endpoint whose peer is outside the copy
  reads as reset (it carries nothing, so it is no flow edge), and a
  channel within the source becomes a new channel the copy's grants name.
  The copies pass the exposure check of D8 like any new cell: a copy
  holding the source's handles is no longer under the source, so a cell
  the source excluded as an ancestor may reach through it.
- A channel endpoint whose peer is outside the copy reads as reset in the
  copy, as inherited connections do today: the peer agreed to talk to the
  source, not to new cells. A channel with both ends in the source is
  copied with its messages, as pipes are.
- The kernel endpoint is inherited.
- The exposure invariant holds after the fork, because the copies'
  egress and edges are the source's.

**Fork from inside** (phase 4, `spawn` of the cell itself). The copy is a
child of the requester. It holds no handle except those the request
passes, after attenuation, and the requester gets a control handle on it
together with a channel if asked.

**Snapshots.**

- Format version 11 (phase 1) adds the tree section: limits and budget
  mode, CPU weight and cap (D23), `on_parent_end`, egress policy and
  ceiling, CPU set, identifier in the domain, label, copy generation,
  `MADV_WIPEONFORK` ranges and the sticky bit of D22.
- Version 12 (phase 2) adds handle records: kind, rights, label, a key
  derived from the kernel's secret (as parked sockets have, finding S15)
  and the boot identity. It also adds the messages queued on the cell's
  endpoints.
- Version 13 (phase 3) adds snapshot views (as files) and live mounts by
  label.
- While a cell is suspended its endpoints stay parked in the kernel, and
  messages sent to it queue up to their bound, as parked connections do.

**Resume in the boot that took the snapshot.**

- The cell comes back under its parent if the parent lives. If the parent
  ended, its `on_parent_end` was applied at that time to the suspended
  cell too.
- Each handle comes back if its key matches, its object lives and it was
  not revoked meanwhile. Otherwise a channel endpoint reads as reset and
  other handles are dropped.

**Snapshot loaded from bytes** (another boot or another machine). The
bytes are untrusted and carry no authority:

- The host chooses placement (`--parent`, root by default).
- Egress becomes the intersection with the new parent's policy, and the
  ceiling likewise.
- Limits must fit the new parent's free budget, as `cap_quota` does today.
- Every handle record is dropped, unless the host binds its label to an
  object of this kernel (`rhizo load file --bind tools=12`), which
  re-derives a grant under the host's authority and passes the D8 check.
- Handles that reference cells absent from this machine are therefore
  dropped by default and re-bound by name only by the host.

**Subtree snapshots** (phase 2): `suspend --tree` and `save --tree` capture
a subtree with the channels between its members as one bundle, so that a
multi-cell agent travels whole. Channels that leave the subtree follow the
rules above.

Alternative: global names for cells so that handles can be resolved on
any machine. Rejected: a global namespace is a channel, and it would let a
forged file name a victim.

## D14. Control protocol and `rhizo`

The protocol goes to version 4 (after `snapshots-on-disk`'s version 3).

Changes to existing requests:

- `run`, `load` and template `fork` gain:
  - `parent` (0: the host);
  - `label`;
  - `limits` and the reservation mode;
  - `egress` and `ceiling`;
  - `on_parent_end`;
  - `cpus` (an exclusive CPU set, roots only, D17);
  - `cpu_weight` and `cpu_max` (D23).
- `list` rows gain the parent, the domain root and the label.

New requests:

- `reparent {cell, to}`, where `to = 0` makes the cell a root;
- `policy {cell, egress, ceiling}`: narrowing for non-roots; for a root,
  any policy;
- `limits {cell, ...}`: within the parent's free budget;
- `grant {object, to, rights, label}`, where the object is a cell, a new
  channel between two cells, or a view `cell:path`;
- `revoke {grant}`;
- `grants {cell}`;
- `scoped {root, request}`;
- `floor {rules}`: the deny floor, sent once after the hello (D7);
- `publish {template, attest}` and `withdraw {template}`: make a template
  a public object, or end that, so that later copies in other domains are
  full copies (D21, D22). The host's file sets, and the templates `rhizo
  up` makes from them, are published at start.

`kill`, `suspend` and `resume` gain a `tree` flag for subtree operations.
New message kinds from the kernel: `audit` (D18) and `approval`
(`add-cell-modules`).

When `--egress` is not given, the egress is `none`. `rhizo chat`,
`rhizo run python-agent` and the agent templates pass `--egress model`
themselves.

**Scoped host access** ("host operations scoped to a subtree"). `rhizo
scope N --socket PATH` opens a second Unix socket on the host. The server
wraps every request arriving on it as `scoped {N, request}`, and the
kernel refuses any target outside `N`'s subtree, as well as `reparent` out
of it and widening. An orchestrator on the host, or an outside service,
can then manage one agent's hierarchy without holding the whole VM. The
kernel checks it, not only the server: defence in depth for the one place
where the host hands out partial power.

**`rhizo` commands.**

```text
rhizo up    --model HOST:PORT [--deny CIDR]... [--allow-host-port PORT]
rhizo run   python --parent 3 --label tools --egress model --mem 256M \
            --cells 4 --on-parent-end cascade
rhizo run   agent --egress model --mem 2G --cpus 2-3     # exclusive CPUs
rhizo audit tail [--cell N] | verify
rhizo ps    --tree
rhizo reparent 7 --to 3 | --to root
rhizo policy 7 --egress allow:tcp:140.82.112.0/20:443
rhizo grant channel 3 7 --label tools [--rights send,recv,carry]
rhizo grant cell 7 --to 3 --rights observe,message
rhizo grant view 3:/data 7:/mnt/data --snapshot | --ro | --rw
rhizo revoke G
rhizo grants [N]
rhizo scope 3 --socket ./agent3.sock
rhizo kill 3 --tree
```

**A cell asking for a child** (phase 4) sends `spawn` on its kernel
endpoint. The request needs:

- the endpoint's `spawn` right;
- a free budget for the child's limits plus one descendant;
- an egress policy within its own;
- the D8 check to pass;
- for a copy of a template, a cell handle with the control right on that
  template.

It is answered with a control handle on the child, and optionally a
channel to it.

## D15. Room for modular cells (out of scope)

Modular cells ("add Python to a running agent") are designed in the
change `add-cell-modules`, in the same pull request. This design gives
them what they need:

- A capability can be a child cell forked from a warm template and reached
  over a channel. Templates are handles (D9), budgets are carved (D4) and
  egress only narrows (D7), so adding a capability can never add authority
  the parent lacks.
- File-only modules can be snapshot views (D11).
- Nothing in the tree, budgets or handles assumes a cell's program or
  files are fixed after creation.

## D16. Phases, and what each costs on the hot paths

Each phase is implemented in this change's tasks, in order. A phase that
is to be archived on its own is first split into its own change carrying
that phase's deltas, so that `openspec/specs` never describes behavior
that does not exist.

| Path | Phase 1 cost | Watched by |
|---|---|---|
| System call entry and exit | None (nothing on the common path reads the tree) | `cell.syscall_ns`, `_insns` |
| Switch between cells | None with one runnable domain per CPU; otherwise a pick among domains by virtual time (D23), measured and justified | `cell.switch_ns`, new `sched.domain_share_pct` |
| End of a slice | Ticks charged to the cell, and to its capped ancestors only (none without caps, D23) | `cell.switch_ns` |
| Fork, template copy | Budget arithmetic, a push to the parent's children list, the copy generation incremented and `MADV_WIPEONFORK` ranges left unmapped in the copy (D22) | `cell.fork_us`, `cell.spawn_from_template_us`, `control.fork_us` |
| Move across domains | A private copy of each shared, non-public page (D21); rare | Tests only |
| Loopback | One network comparison per socket looked at; the two-cell figures become two threads of one cell (D5), a redefinition justified in the pull request | `cell.loopback_rtt_us`, `cell.tcp_resume_us` |
| Audit records | One record copied into a per-kernel ring for each tree change and denial (none on system calls that change nothing) | `cell.fork_us`, `cell.grant_us` |
| Outside connect, UDP send | Rule scan | `net.tcp_rtt_us`, new `net.egress_check_insns` |
| File-page fault | One more word in the page-cache key (the domain, or 0 for a public page, D21) | `workload.dynamic_*`, `workload.node_*` |
| File writes | A meter charge per new block | `cell.file_write_read_us`, `cell.fork_with_files_us` |
| Idle memory | At most 48 bytes per cell for the node, plus 16 for the copy generation, CPU weight and cap | `cell.idle_bytes`: grows, justified in the pull request |
| Published file sets | A frame per page of their mapped files while published (D21) | No cell metric; the `publish` reply reports the frames |
| Snapshot size | Tree section, about 90 bytes, plus 16 per `MADV_WIPEONFORK` range | `cell.suspended_bytes`: grows, justified |

Phases 2 to 4 add paths of their own (channels, views, cells creating
cells) with their own metrics. They change none of the paths above.

## D17. Exclusive CPU sets per domain (production option)

This is the user's decision 4. Timing channels through the scheduler and
the core-private caches (L1, L2, TLB, branch predictors) are the strongest
channels left (D12, items 15 and 16). For deployments that need them gone,
the host can give a root an **exclusive CPU set**:
`rhizo run --cpus 2-3`, or `cpus` on `run`, `load` and `reparent` to root.

**What the kernel guarantees.**

- Every cell of the domain runs only on the set's CPUs.
- No cell of another domain runs there, whether placed, stolen or
  balanced: the `rhizo-sched` run queues of those CPUs take only that
  domain's cells.
- A domain without a set runs only on CPUs no set holds (the shared CPUs).

**Rules.**

- The boot CPU is never exclusive: it polls the card and the serial line,
  and runs the control loop.
- At least one CPU always stays shared.
- Sets of different roots never overlap.
- A set is given only to a root, and the whole domain inherits it. A
  descendant cannot leave it or widen it.
- Moving a cell across domains (host only) moves it to the new domain's
  CPUs.

**What it does not remove.**

- The last-level cache and memory bandwidth, shared by all cores (on
  hosts that have cache allocation technology, the VMM can partition the
  cache below Rhizo; that is outside the kernel).
- The kernel's global locks (D12, item 28), taken by system calls on
  every CPU.
- The network card, polled by the boot CPU.
- The virtual CPUs' own placement on host cores: the VMM must pin each
  vCPU to its own physical core with no sibling hyperthread shared with
  another domain's vCPU, or the hardware shares again what the kernel
  separated. The operator documentation says so, and `rhizo up --pin`
  pins the vCPUs where the VMM supports it (QEMU and Cloud Hypervisor
  through their affinity options).

**Cost, stated plainly.**

- **Density.** The set's CPUs stay idle when its domain has nothing to
  run: the set is reserved capacity, like a reserved memory budget. A VM
  with 8 vCPUs and two exclusive domains of 2 CPUs leaves 3 shared CPUs
  for everyone else.
- **Latency.** Work is never stolen across the set's edge. A burst in a
  domain waits for its own CPUs, even with other CPUs idle.
  `cell.parallel_us` and `cell.scaling_pct` are measured with and without
  sets.
- **System calls and switches cost nothing more.** The set is checked
  when a cell is placed or stolen, not when it runs.
- **Interrupts.** Wake-ups between CPUs stay as today. The set changes
  only which run queue a cell may join.

**How it is checked.** `covert.cpu_bps` is measured twice: two domains on
shared CPUs, and two domains with exclusive sets of one CPU each. The
second figure should be at noise level, and both are published in
`docs/security.md`. Hostile test `exclusive_sets_hold`: a cell of a domain
without a set never runs on a set's CPU, checked by the scheduler's
per-CPU record over a busy run with stealing.

Alternative: fixed time partitions per domain (seL4's domain scheduler:
every domain gets a fixed slot in a fixed cycle whether it uses it or
not). Not designed: it closes the scheduler channel on shared CPUs too,
but it wastes CPU and adds latency for every domain. Exclusive sets give
the same separation where it matters, at a cost the operator chooses
domain by domain.

**As built in phase 1 (task 1.9a).**

- **Classes of CPUs.** `rhizo_sched::Sets` splits the CPUs into classes:
  each set is one, and the CPUs no set holds (the shared CPUs) are
  another. Each cell of a domain with a set is given that set, by key;
  every other cell, the shared CPUs. The scheduler queues a cell only on a
  CPU of its class (`place` picks the shortest such queue, a wake-up or a
  cap's release onto another class's CPU goes to the shortest of its own,
  and a cell put back balances only within its class), and a CPU steals
  only from the queues of its class and joins only gangs of cells it may
  run. So every queue holds cells of one class, and the pick by weight
  (D23) runs unchanged on the queue the class gives: sets restrict where,
  weights decide who.
- **"At least one CPU stays shared" means one besides the boot CPU**, as
  `docs/cell-hierarchy.md` says: the boot CPU polls the card and the
  serial line and runs the host's operations, so the other domains keep a
  CPU that does only their work. `Sets::fits` (no CPU 0, only CPUs the
  machine has, no other root's CPU, and a shared CPU left beside CPU 0) is
  checked by `Host::place` for a root (protocol version 4, `run`, `load`
  and `fork` with `--cpus`): a refused set is `Invalid`, and the request
  changes nothing. On fewer than three CPUs no set fits. The control
  kernel audits the refusal as a tree denial, as every refused tree
  request (task 1.9b).
- **Re-parenting.** A root moved under a parent leaves its set
  (`rhizo_cap::Tree::reparent`, through `narrowed`): its subtree runs on
  the new domain's CPUs, and gets no set back if it is made a root again.
  Protocol version 4's `reparent` carries no CPU set, so a set comes only
  with a placement; giving one on `reparent` to root waits for a protocol
  version that has the field.
- **When the sets take effect.** The host computes them from the tree in
  `Host::reshare`, with the shares, on every tree change while a set or a
  weight is in use (one field tested otherwise, as D23 built it). Cells
  queued where they may no longer run move at once; a cell running on
  such a CPU moves when its slice ends (at most two timer ticks).
- **Waking the right CPUs.** A cell made runnable may now be for another
  class than the CPU that woke it. With sets, each locked section that can
  make cells runnable (taking a cell, giving one back, a host operation)
  wakes the lowest idle CPU of each class with queued cells
  (`Scheduler::wanting`), instead of the lowest idle CPU.
- **Cost.** With no set, round robin pays one test of the sets in each of
  `place`, the put back (`back_to`), `push` and a steal, and the host one
  in taking a cell and one in giving it back, each only while another CPU
  idles; the work with sets is out of line. Placement, a put back and a
  wake-up with sets search the sets by key (binary search) and scan the
  queues of the class: linear in the CPUs. `Scheduler::next` and its wake
  of due sleepers are marked inline, as the compiler inlined them before
  (a second caller made it call them, +21 instructions per switch), and
  `Host::resume` no longer lets its threads waiting on descriptors look
  again twice (`file` does it for a cell new to the scheduler). Measured
  with `--icount-only` against `main` (6b1ea42), x86_64 then AArch64:
  `cell.switch_insns` 878 to 869 and 831 to 826, `cell.switch_domains_insns`
  1,376 to 1,370 and 1,298 to 1,289, `cell.resume_insns` 4,352 to 4,324
  and 11,777 to 11,759, `cell.spawn_insns` +1 and +5, `cell.run_insns`
  +42 and +17 (+0.05 %); every other figure within ±0.25 % (no increase above +0.07 %).
- **`rhizo up --pin`** pins each vCPU thread of the VMM, found by the name
  QEMU (`CPU <n>/KVM`, `CPU <n>/TCG`) and Cloud Hypervisor (`vcpu<n>`)
  give it, to the first allowed host CPU of a physical core of its own
  (cores told apart by `core_cpus_list`), with `sched_setaffinity`
  (`libc`, MIT OR Apache-2.0, already a dependency of `xtask`). That needs
  no VMM option, works the same on both VMMs, and refuses a VM with more
  vCPUs than the host has cores. Firecracker is not covered (its vCPU
  threads are named otherwise and the design does not ask for it).
- **Tests.** `cargo test -p rhizo-sched exclusive_sets`: a property over
  any interleaving of CPUs taking, preempting, putting to sleep and waking
  cells and of the host taking them out, putting them back and changing
  the sets, round robin and by weight: no cell runs or is queued outside
  its class, and a CPU (outside a set too) idles only when nothing it may
  run is queued, and `wanting` then reaches it; a busy machine where every
  CPU always has work it may run; queued cells moving when the sets
  change; `fits`; gangs. In-kernel `exclusive_sets_hold` (4 vCPUs,
  `ktest-smp`): a domain with CPUs 2 and 3 (a spinner, and a cell
  spreading three threads) runs only there and on both, while eight
  spinning roots, one of weight 300, and a cell spreading four threads run
  only on CPUs 0 and 1, and on both; refused sets (CPU 0, absent CPUs, no
  shared CPU left, overlap, a child's set); on one CPU (`ktest`) every set
  is refused.
- **`covert.cpu_bps`** with and without sets is left to task 1.13, with
  the colluder cells of task 1.12: it needs a sender and a receiver cell
  and a benchmark of its own in `cargo xtask bench`.

## D18. Audit log

Operators must be able to say after the fact who granted what to whom,
which cell was moved where, and which cell tried to reach what. The log
lives on the host, is append-only, and is fed by the kernel.

**What the kernel records**, each with the kernel's sequence number, the
time and the cells by host identifier and domain:

- cell created (by whom, where, with which limits, egress, ceiling and
  CPU set) and ended (how);
- re-parented and promoted cells;
- policy, ceiling and limit changes;
- grants (object, rights, label, grantor, holder, parent grant),
  transfers and revocations;
- refusals by the exposure check (D8) and by authority checks (D2);
- egress denials by policy or floor (destination, protocol);
- kernel-endpoint requests refused;
- requests from scoped sockets (D14), with the scope;
- floor changes and host exceptions to it;
- module approvals and module loads (`add-cell-modules`).

**How it gets out without hurting cells.**

- The kernel copies each record into a ring of 64 KiB and sends records
  to the host in `audit` frames, as it sends output.
- A cell never waits for the log. When the ring is full, records are
  counted as dropped, and the next record carries the count.
- Egress denials are folded: at most one record per cell and second,
  carrying the count of further denials and the first destination. A cell
  that tries thousands of addresses cannot flood the log.

Records go only to the host. No cell can read the log, so it is not a
channel.

**How the host keeps it.**

- The `rhizo` server appends each record as one JSON line to
  `$RHIZO_STATE/audit/audit-<boot>.jsonl` (`~/.rhizo/audit/` by default,
  `/var/log/rhizo/` in a system install). The file is opened with
  `O_APPEND`, written before the record is acknowledged to anyone, and
  synced every second and at `rhizo down`.
- Each line carries the SHA-256 of the line before it, which makes a hash
  chain across files. When a file reaches 64 MiB a new one is started,
  whose first line names the previous file and its last hash.
- `rhizo audit verify` walks the chain and reports:
  - an edited, removed or reordered line;
  - a gap in the kernel's sequence numbers (records lost between kernel
    and host);
  - the kernel's own drop counts.
- `rhizo audit tail [--cell N] [--domain N] [--kind K]` follows the log.
- For an external store, the operator forwards the files (journald,
  syslog, a SIEM). The chain still detects tampering on the host. Writing
  to an append-only remote is the operator's step, documented.

**Limits.** The hash chain shows tampering. It cannot prevent a host
administrator from deleting the whole log, which is why forwarding is
recommended for production.

**As built in phase 1 (task 1.9b).**

- The record and its ring are in `rhizo-control` (`audit`), so the host
  tests them: a record is its sequence number, the kernel's time, a kind,
  the cell and the root of its domain, a count, the records dropped
  before it and a detail for people (at most 200 bytes). The ring is one
  `VecDeque` of 64 KiB of encoded records, allocated at the first record
  and never grown; time is passed in. The kernel keeps one ring for all
  CPUs behind a `Global` (`kernel/src/audit.rs`), and records cells by
  kernel identifier; the control kernel gives the host's identifiers, and
  the domain's root where the record did not name it, when it sends.
- Records leave as `audit` frames of version 4 (kind `0x85`, identifier
  0), which D14 lists; no request was needed: the control kernel sends
  what the ring holds after each turn of its loop, and its idle loop wakes
  for it. A frame of version 3 of that kind is refused.
- Every record takes a sequence number, dropped ones too, so the host sees
  a gap as well as the next record's drop count. Denials (egress, scope,
  tree, load narrowed, publish refused) fold per cell, kind and second:
  the first goes in at once, and once that second is over one more record
  carries the count of the others and the first one's detail; so the
  counts of a cell's records add up to its denials. At most 8 cells fold
  at once; a ninth flushes the oldest early.
- Recorded in phase 1: cells the host made (`run`, `fork`, `load`, with
  their placement) and their ends; moves, policies and limits; requests of
  scoped clients, let through or refused, with the scope; tree refusals
  (`Denied`, `Invalid`); a load whose quota was cut; a publication refused
  or made on the host's attestation (`Host::may_publish`); a vsock
  connection to the control channel refused for want of the secret
  (`auth-refused`, change `authenticate-control-channel`). Egress denials
  come with task 1.6 through `audit::deny(Kind::Egress, cell, domain,
  detail)`; grants with phase 2.
- On the host the files are `audit/audit-<n>.jsonl` (`n` counting up from
  0, so names sort in order), a new one at each `rhizo up` and at 64 MiB.
  Each file's first line names its boot and the file before; each line's
  `prev` is the SHA-256 of the line before, the first file's first line
  after 32 zero bytes. `audit/head` holds the last line's file and hash,
  so a cut end or an edited last line shows as well. `rhizo down` keeps
  `audit/` (spec `control-plane`, state after down).
- SHA-256 is written out in `rhizo` (about 60 lines) rather than taken
  from a crate, which would add seven crates to the binary for one
  function. Tests check it against the FIPS 180-4 vectors, and against
  RustCrypto's `sha2` (MIT OR Apache-2.0, a dev-dependency only) at every
  padding boundary (0, 55, 56, 63, 64, 65, 119, 120, 128 bytes) and on
  arbitrary inputs up to 1,000 bytes. The JSON lines use `serde_json`,
  already in the workspace through `xtask`.
- What `verify` proves, and what it does not. The chain is unkeyed: it
  detects an edit, removal or reordering of part of the log by someone
  who does not rewrite everything after it. Anyone with write access to
  the state directory can rewrite the whole chain from the edited line
  on, and `audit/head`, and `verify` then passes. What it cannot hide
  while a VM runs: the kernel's sequence numbers and drop counts, so
  records missing from a live session show as a gap unless the rest of
  that session is rewritten too. A possible later mitigation, not built:
  export `head` (or each file's last hash) to a sink outside the host's
  reach, or sign it with a key kept outside the state directory.
  `docs/security.md` (audit log) says the same.
- `rhizo audit tail [--cell N] [--domain N] [--kind K] [--follow]` shows
  records, `--follow` then polls for new lines; `rhizo audit verify`
  names each edited, removed or reordered line, each gap in sequence
  numbers, and the kernel's drop counts, and fails on any break.

Alternative: keep the log in the kernel and let the host read it.
Rejected: kernel memory is bounded, and the log must outlive the VM.

## D19. Limits and failure modes

Every structure the hierarchy adds is bounded, and every failure leaves
the tree as it was.

| Limit | Value | When exceeded |
|---|---|---|
| Tree depth below the host | 16 | `EMLINK`, nothing made |
| Children of one cell | 1024 (and its descendant budget) | `EDQUOT` from the budget, `EMLINK` past the hard cap |
| Cells in the kernel | The cell table's capacity (descendant budgets admitted against it, D4) | Root refused by the host |
| Handles per cell | 256 | `EMFILE` |
| Derivation chain of a grant | 16 | `EMLINK` |
| Channels a cell holds | 128 | `EMFILE` |
| Channels waiting in an inbox | 64 | `ECONNREFUSED` for the opener |
| Message | 64 KiB and 16 handles | `EMSGSIZE`, `ETOOMANYREFS` |
| Queue per endpoint | 64 messages, 256 KiB | `EAGAIN` or block |
| Rules per egress policy | 64 | `EINVAL` |
| Models one accelerator serves | 64 (one bit each in a handle, D24) | Refused by `rhizo up` |
| Deny-floor rules | 64 | Refused by `rhizo up` |
| Label | 64 bytes, `[A-Za-z0-9._-]` | `EINVAL` |
| Pending module approvals per cell | 1 (`add-cell-modules`) | `EBUSY` |
| `MADV_WIPEONFORK` ranges per cell | 64 (D22) | `ENOMEM`, as Linux when a mapping cannot be split |
| CPU weight; CPU cap | 1 to 10,000; at most the parent's (D23) | `EINVAL`; `EACCES` for a cap above the parent's |
| Audit ring | 64 KiB | Records dropped and counted, never blocking |

The values are constants in `rhizo-cap`, checked against one another at
compile time. A handle at its bound, for instance, still fits in a
cell's descriptor quota.

**Failure modes.**

- **Atomicity.** A failing tree operation changes nothing:
  - a fork into `n` copies is all or none;
  - a re-parent checks budget, depth, egress and the exposure check
    before changing anything;
  - a grant is checked, then applied in one step under the tree's lock.

  The tests run each operation with every allocation inside it made to
  fail in turn (a failing frame allocator, as the hostile tests already
  do), and check the tree, budgets and grants afterwards.
- **Kernel memory exhausted.** A tree operation fails with `ENOMEM`, or
  the host request with `NoMemory`, and changes nothing. Running cells are
  untouched.
- **Host link lost.**
  - Cells keep running.
  - Requests that wait for the host (module approvals) are refused at
    their deadline.
  - Audit records fill the ring and are then counted.
  - Output is kept as today (`KEPT_OUTPUT`).

  Nothing in the tree waits for the host.
- **A parent crashes.** It is killed with a fault report, and
  `on_parent_end` applies as for any end.
- **A refused snapshot or module** keeps nothing (as `load` today).
- **Invariant checks.** In test builds `rhizo-cap` checks its invariants
  after every operation:
  - budgets sum up;
  - no cycles;
  - every handle's object lives;
  - control never flows up;
  - every exposure is within its ceiling.

  The property tests drive random operation sequences against them.

## D20. Upgrade path for snapshots and the protocol

Production hosts keep snapshot files and templates across kernel
upgrades, and the version bumps of this design (snapshots 11 to 13 and 14
in `add-cell-modules`, protocol 4) must not strand them.

**Snapshots.**

- The kernel reads the current format **and the one before it**. It
  writes only the current one.
- Reading the previous version fills new fields with their safest
  values: egress `none`, ceiling `none`, no handles, no views, no CPU set,
  limits capped to the new parent's, placement by the host.
- Older files go through `rhizo snapshot upgrade IN OUT`, a host tool
  built from the same `rhizo-snapshot` crate. It converts one version at
  a time (10 → 11 → 12 → 13 → 14) with one function per step, each tested
  on a recorded file of its source version, kept in
  `crates/rhizo-snapshot/tests/data/`.
- The upgraded file is checked by `from_bytes` before it is written.
- An upgraded module file has a new digest, so it must be signed again
  (`rhizo module resign`, `add-cell-modules`). The conversion is
  deterministic, so two operators converting the same file get the same
  bytes.
- Downgrades are not supported: a kernel refuses a newer file and names
  both versions.

**Protocol.**

- The kernel's hello names its version. `rhizo` speaks the current
  version and the one before it, so a newer `rhizo` can still drive a VM
  booted before an upgrade, save its cells and bring it down.
- A kernel of any other version is refused with both versions and the
  upgrade steps.

**Upgrading a running deployment:**

1. `rhizo suspend --tree` each root, then `rhizo save` each.
2. `rhizo down`, then install the new kernel and `rhizo`.
3. `rhizo up`.
4. `rhizo snapshot upgrade` the files if they are two versions old, then
   `rhizo load`.

Parked connections reset, as across any boot. The audit log notes both
boots.

Every version bump from now on adds:

- its upgrade step;
- a recorded file of the old version;
- a test that loads the old file in the new kernel.

The tasks of each phase carry these.

## D21. Content sharing stays inside a domain

D21 to D24 come from a review of the state of the art that the user
approved.

**The rule.** A frame is shared by content (the same bytes, reached from
two places, mapped from one frame) only between cells of one domain. The
domain's identifier is part of every content key, as vLLM's `cache_salt`
is part of its prefix-cache key. This covers:

- the page cache of mapped file pages, executables and interpreters
  included;
- the frames a suspended cell's snapshot refers to instead of copying
  (`add-shared-snapshots`);
- any later cache keyed by content: a KV or prompt cache of an inference
  service, or a device's cache (`add-accelerators`).

Across domains, only **public objects** are shared: immutable, holding no
secret, and published by the host. They are:

- the executables and libraries of the host's file sets;
- read-only template files and read-only index files;
- published templates (D22), signed module templates among them
  (`add-cell-modules`).

They are shared only when they are loaded, by their identity. **Nothing
is ever shared by scanning memory** for equal pages (no merging like
Linux's KSM), inside a domain or across domains.

**Why.** Each of these attacks used content sharing across a trust
boundary:

- KSM page merging combined with Rowhammer gave Flip Feng Shui (USENIX
  Security 2016, https://vusec.net/projects/flip-feng-shui): the
  attacker writes a page equal to the victim's, the merge puts the
  victim's data in a frame the attacker chose, and a bit flipped there
  changes the victim.
- Deduplication timing breaks ASLR in neighbouring VMs
  (https://www.nebelwelt.net/files/15WOOT.pdf): a write to a merged page
  is slower, which tells whether a guessed page exists elsewhere.
- Prompt-cache timing leaked prompts between users at 8 of 17 API
  providers (ICML 2025, https://arxiv.org/abs/2502.07776). OpenAI and
  Anthropic now isolate their caches per organization.

The domain is the unit the host isolates (D12), so it is the unit of
sharing. Public objects are the same for everyone and hold no secret:
sharing them tells a cell only which public object it uses, and an
attacker cannot choose what lands in a public frame.

**Residency tells nothing either.** A hit or a miss on a public page
would still say whether another domain mapped it first. So the kernel
loads every page of a public object's mapped files when the host
publishes it, and keeps them until the host withdraws it and no mapping
is left. A cell's map of a public page always hits, and no cell can
evict one. A public frame is never sole-owned by a cell, since the
publication holds a reference (item 12 of D12).

**Moves across domains.** A cell that leaves its domain (promotion, or a
move by the host) first takes private copies of the pages it shares with
its old domain, public pages excepted. The move is refused with `ENOMEM`
if its budget cannot hold them. A host fork whose copies go into another
domain makes full copies, unless the source is a published template.
This replaces the residual coupling that D2 used to accept.

**What the implementation will change** (nothing is done here). Today
`rhizo-abi`'s `PageCache` keys pages by `Page::key()`, the address of
the file block's shared allocation and the length, in one cache for the
whole kernel (`PAGES` in `kernel/src/cell.rs`). Copy-on-write frames
(`rhizo-mm`'s `cow`) and the frames a snapshot refers to
(`rhizo-snapshot`'s `Data::Frame`) are shared by lineage: fork, template
copy, suspend. No code merges frames by comparing contents. The changes:

- `rhizo-fs`: `Page::key()` stays the block's identity. A file gets a
  `public` flag, set on every file of a published set or template and
  cleared by any write or truncation (a write already gives the file a
  new block); `Page` carries it.
- `rhizo-abi` (`pages.rs`): `PageCache` keys pages by
  `(scope, block, length)`, where `scope` is the caller's domain
  identifier, or 0 for a public page; `share` takes the scope. The limit
  and the search for dead pages are counted per scope, and a domain's
  pages are charged to its root (D4). A new `publish` call fills and
  pins a public object's pages; they are never filled on demand.
- `kernel/src/cell.rs`: the calls into `PAGES` pass the cell's domain
  identifier, a 64-bit number held in its tree node and never reused
  within a boot.
- `rhizo-mm` (`cow`): fork and write resolution do not change: they share
  by lineage only. For a move across domains, the kernel walks the
  cell's tables and resolves every shared page that is not public as a
  write would, without writing.
- `rhizo-snapshot`: a frame record may refer only to a frame of the
  cell's own domain or of a public object. Moving a suspended cell across
  domains turns its other frame records into private frames, as `export`
  writes contents in their place. Delta records of module files ("same as
  base page N", `add-cell-modules`) are honoured only against a public
  base.

**Cost.**

- **Memory.** Sharing of private data across domains is lost: two
  domains that map the same private file each keep a copy, a host fork
  into another domain copies, and a cell that leaves its domain copies
  what it shared. Libraries, executables and published templates stay
  shared, so a library mapped in two domains still takes one copy (item
  9 of D12 had accepted two).
- **Published sets keep a frame for every page of their mapped files**
  (their ELF files, and the files the host marks as mapped) while
  published, not only for the pages someone touched. Their bytes are
  already in memory as file blocks; the frames are the second copy that
  mapping needs. They are charged to the kernel's reserve and admitted
  like a reserved root.
- **Time.** One more word in the page-cache key (`workload.dynamic_*`,
  `workload.node_*`), and a copy proportional to the shared pages on the
  rare moves across domains.

Alternatives:

- Share by content everywhere and add noise to the timing. Rejected:
  noise lowers a channel's bandwidth without closing it, and does nothing
  against Rowhammer.
- No sharing across domains at all. Rejected: every domain would hold its
  own CPython and libc, and density is the project's point.

## D22. Randomness after fork, resume and template spawn

The kernel already gives each cell its own generator, split off the
kernel's on first use and never copied (D2 of `add-crypto-random`), so a
copy's `getrandom` never repeats its source's. But generators in user
space live in the cell's memory, and a fork, a template copy or a second
load of one snapshot copies them:

- CPython's `random` module, seeded once at import, and its string hash
  seed;
- Node.js's `Math.random` (V8 seeds it once per isolate) and the OpenSSL
  generator behind `crypto`;
- glibc's stack-protector value and pointer guard, taken from
  `AT_RANDOM` at start, and the address layout.

Two copies of one template then draw the same `random.random()`
sequence. Firecracker's guidance says the same of VM clones
(https://github.com/firecracker-microvm/firecracker/blob/main/docs/snapshotting/random-for-clones.md):
the guest kernel reseeds through VMGenID, and user space has to be told.
Linux tells it in two ways: pages marked `MADV_WIPEONFORK` read as zero
in a child (BoringSSL relies on this), and `vgetrandom` in the vDSO
checks a generation counter the kernel bumps when the VM is cloned.

**A copy generation per cell.** Each cell has a 64-bit counter, 0 at
spawn, incremented in each new copy: every copy of a host fork or of a
template, every load of a snapshot file and every resume. The source of
a fork keeps its value, as a parent does on Linux. Only these events
write it, so it tells nothing of other cells. User space learns of a
copy in three ways, one per kind of program:

1. **`/run/rhizo/generation`**: a read-only file of 8 bytes (little
   endian) in the cell's own tree, for interpreters and scripts. It is a
   cell's VMGenID. Reading it is one system call, and runtimes check it
   once per request, not per random number.
2. **`MADV_WIPEONFORK`**, honoured as on Linux, with "fork" meaning any
   copy above: in the copy, a wiped range maps nothing and reads as zero
   on first touch. No copy is made, and cells that never ask pay nothing.
   Unmodified libraries that use it (BoringSSL) reseed. `MADV_KEEPONFORK`
   undoes it, and the ranges are cell state.
3. **A copy's `getpid` differs from its source's**: numbering inside the
   domain skips the source's identifier. OpenSSL 3 and CPython's
   `tempfile` detect a fork by a change of `getpid`, so they reseed.

No vDSO is added: cells have none, and a system call per request costs
far less than an agent's turn.

**What the shipped templates do.**

- **CPython** (`cells/python/agent_demo.py`, `rhizo_demo.py`, the
  `python-agent` template and module servers): after the warm-up pause
  in which the host freezes them, they compare `/run/rhizo/generation`
  with the value read during warm-up, and on a change call
  `random.seed()`, which draws from the copy's own `getrandom`. The
  string hash seed cannot change in a running interpreter, so every copy
  of a template shares it. Each copy can read it in its own memory
  anyway: this is documented, not fixed, and a server that hashes
  untrusted keys from another domain starts from its own program rather
  than from a template.
- **Node.js** (`cells/node/demo.js`): V8 cannot reseed `Math.random`.
  After a copy, the template replaces it with xorshift128+ (V8's own
  algorithm) seeded from `crypto.getRandomValues`. `crypto` itself
  reseeds through OpenSSL's `getpid` check (way 3).
- **C programs**: glibc's `arc4random` calls `getrandom` and keeps no
  state, so they need nothing. The stack-protector value, the pointer
  guard and the address layout stay the template's, as for every fork
  server (Android's zygote has the same weakness). `docs/security.md`
  says so.

**Templates hold no secrets.** A template's memory and files are copied
into every child, and a published template's into other domains.
CodeSandbox refuses memory snapshots of VMs that hold secrets for the
same reason. Rules:

- The host passes a secret (a model key, a token) to a copy after the
  copy, through its input or a channel, never to a cell before it is
  frozen. `rhizo` warns when a template is made of a cell that received
  host input after its start.
- The kernel keeps a sticky bit per cell, set when the cell connects
  through the network card or receives a message, a handle or a view.
  Publishing a template whose bit is set is refused, unless the host
  attests that it holds no secret (signed modules, after the operator's
  review). The attestation goes into the audit log (D18).
- A builder's per-build token (`add-cell-modules`) is revoked by the
  package proxy before the builder freezes itself, so its template holds
  only a dead token.

**Tests** (behavior, both architectures):

- `copies_draw_different_random`: two copies of a CPython template frozen
  after `import random` print `random.random()` and `os.urandom(16)`;
  the copies print different values. The same for two loads of one
  snapshot file, and for a Node.js template's `Math.random()` and
  `crypto.randomUUID()`.
- `wipeonfork_reads_zero_in_copies`: a C cell marks a page
  `MADV_WIPEONFORK` and writes 1 in it; its fork, a copy of its template
  and a resume of its snapshot read 0, and the source still reads 1.
- `generation_changes_only_in_copies`: a fork leaves the source's
  `/run/rhizo/generation` unchanged, and each copy reads another value.
- `copy_pid_differs`: a copy's `getpid` is never its source's, in the
  same domain or as a new root.
- `tainted_template_not_published`: a template of a cell that connected
  to the host's echo server is refused publication without attestation.

**Cost.** 8 bytes per cell and in the snapshot's tree section; the wipe
ranges in snapshots, like the write-or-execute ranges. Nothing on system
calls, switches or forks of cells that never use `MADV_WIPEONFORK`.
Templates pay one file read per request.

**As built in phase 1 (task 1.8b).**

- The generation and the sticky bit are fields of the cell (the bit fits
  in its padding). The wipe ranges share the one word the cell already
  keeps for its write-or-execute ranges, so a cell without either still
  pays one word.
- Opening `/run/rhizo/generation` gives a file that no name refers to,
  holding the value at the time of the open. It is matched by that
  absolute path only, until the `/run/rhizo` directory of phase 2 (task
  2.4) holds it.
- A fork or a copy of a template replaces the pages of its wipe ranges with
  zeroed frames that keep the same permissions. A snapshot holds those
  pages as zeros, so no resume or load ever gets their contents, and no
  file does either.
- Until the tree section of format version 11 (task 1.10) holds them, the
  generation, the sticky bit and the wipe ranges stay beside a snapshot in
  the kernel. A cell loaded from a file gets a generation drawn from the
  kernel's generator (never 0, below 2^62), not a count: the file does
  not say what the cell saw, and a template that was itself a copy saw 1.
  Counting would let two loads of it miss the change and draw the same
  numbers. The loaded cell has no ranges and the bit clear.
- The bit is set when a socket call of the cell reaches the network card.
  The kernel's publication check refuses a template whose bit is set,
  unless the host attests. Publishing (task 1.8a), the audit record (task
  1.9b), and setting the bit for messages, handles and views (phases 2 and
  3) come with their tasks.

Alternatives:

- The kernel reseeds user-space generators by writing into them.
  Rejected: it would have to know every runtime's memory layout.
- A page holding the generation, mapped into every cell. Rejected: 4 KiB
  per cell on `cell.idle_bytes` for what the file and `MADV_WIPEONFORK`
  already give.

## D23. Hierarchical CPU and memory quotas: accounting and fairness

D4 gives every subtree reserved limits on memory, CPU time, sockets and
descendants. This decision states what accounting them costs, and how
CPUs are shared between subtrees while they run.

**Accounting never walks the tree.** Because children are reserved, a
use is charged only to the cell that makes it: one addition and one
comparison in its meter. cgroup v2 charges every level of the hierarchy
on every page; Rhizo pays that walk once, when a child is made or ends
(D4).

- Pages, page tables and file blocks: one charge per frame or block, at
  allocation.
- Socket queues and `smoltcp` buffers: charged at their full size when
  the socket is made, never per packet.
- CPU time: ticks charged to the cell at the end of each slice, as today.
  A subtree's total is bounded by its carving, so no running sum is kept;
  `rhizo ps --tree` adds a subtree's figures when asked.

**CPU shares between domains.** Today each CPU's run queue is round robin
over cells, so a domain with 100 runnable cells gets 100 times the CPU of
a domain with one: a denial of service between domains. After this
change:

- Each node has a **CPU weight** (1 to 10,000, default 100). Domains
  share the CPUs by their roots' weights; inside a domain, siblings share
  their parent's part by theirs.
- **Two levels exact, deeper levels flattened.** Each CPU's queue picks
  the domain with the least virtual time (CPU time used divided by its
  weight), then that domain's cell with the least virtual time scaled by
  its flattened weight: the product of its shares at each level, computed
  when the tree changes, never when a cell runs. When part of a domain is
  idle, its share goes to the rest of the domain in proportion to their
  flattened weights. Fairness between domains is exact; inside a deep
  domain it is approximate.
- **One domain costs nothing more.** When a CPU has one runnable domain,
  the first level is skipped. `cell.switch_ns` stays gated, and the pull
  request justifies the domain level with its measured cost.
- Balancing between CPUs moves cells as today. Fairness holds per CPU and
  over balancing periods, as with Linux's CFS.

**CPU rate caps.** A node may have a **CPU cap** (`--cpu-max 1.5`: at
most 150 ms of CPU per 100 ms period, for its whole subtree). A child's
cap is at most its parent's, and caps only narrow, as egress does. A
subtree that used its cap leaves the run queues until the next period.
Each cell keeps the list of its capped ancestors, computed when the tree
changes, so the end of a slice charges only those, and a tree without
caps pays nothing.

**Memory needs no fairness rule.** Memory is reserved, there is no swap
and no reclaim between cells, and the kernel's caches take only memory no
reservation holds (D4).

**Not offered: a guaranteed CPU share.** Another domain's load changes
how much CPU a cell gets at a given moment: that is the timing channel of
item 15 of D12, accepted. A domain that needs CPUs no one else touches
gets an exclusive CPU set (D17).

**Tests.**

- `cargo test -p rhizo-sched fair_shares`, with properties: with weights
  `w_i` and every domain runnable, each domain's share over 1,000 slices
  is within one slice of `w_i / sum(w)`; an idle domain's share goes to
  the others; a capped subtree never exceeds its cap by more than one
  slice per period.
- In-kernel `domains_share_by_weight`: on one CPU, a domain of 16
  spinning cells and a domain of one, with equal weights, each get half
  the CPU within 5 % over 2 s.
- In-kernel `cpu_cap_holds`: a subtree capped at 0.25 CPU gets at most
  25 % plus one slice per period.

**Benchmarks.** `sched.domain_share_pct` (the one-cell domain's share in
the test above, reported), and no regression of `cell.switch_ns`,
`cell.parallel_us` or `cell.scaling_pct` with one domain.

**Snapshots and protocol.** Weight and cap are limits: they go into the
tree section (format version 11) and into `run`, `load`, `limits` and
template `fork` (protocol version 4, `--cpu-weight` and `--cpu-max`).

**As built in phase 1 (task 1.9c, kernel wiring).**

- **When the host shares by weight.** Round robin per cell already gives
  each domain exactly its weight's part while every node is a root (alone
  in its domain) of the default weight without a cap, which is every cell
  until the tree says otherwise. So the host stays round robin then, and
  shares by weight as soon as one node is not plain (a child, a weight
  other than 100, a cap), and goes back to round robin when none is left.
  The scheduler's policy is `Option<Fair>`: round robin while `None`.
- **Cost of the choice.** Each locked section of the scheduler tests the
  policy once: taking a cell (`next`), and putting it back (charging and
  requeueing in one call, `requeue_charged`; a filed cell is charged
  apart). A copy of a plain cell, a root of default weight and a resumed
  cell keep the host round robin, so spawn, fork and resume test the policy
  without computing anything: the host keeps it in a field of its own
  (`Host::by_weight`), so a tree change on round robin tests that field,
  takes no lock and calls nothing, and the scheduler's `place`, `requeue`
  and `push` stay inlined into their callers with the by-weight path out
  of line. Measured with `--icount-only` against `main` (e655beb):
  `cell.switch_insns` 875, unchanged (x86_64), and 817 to 819 (AArch64);
  `cell.resume_insns` +5 and -1; `cell.spawn_insns` +78 (x86_64, +0.28 %:
  code generation around the spawn path, whose helpers the tests' and
  benchmarks' placements now also call) and +2; `cell.fork_insns` +22 and
  +16; every other figure within +0.53 %.
- **No search, no allocation per switch.** While the host shares by
  weight, the scheduler tells `Fair` each entry it queues and each it
  takes out other than by a pick, so a pick never reads the queue:
  - each CPU keeps the list of domains with work in its queue, and each
    domain one binary heap of its queued cells per CPU, least finishing
    virtual time first; a pick scans the CPU's domains (WF2Q+, linear in
    the domains with work there, which a CPU's queue rarely holds many
    of), pops the chosen domain's heap (O(log k) in its cells there), and
    finds the entry in the queue by its arrival number (the first, or a
    binary search: a queue is in arrival order);
  - cells, domains and caps are vectors sorted by identifier (binary
    search on a tree change or a lookup by key); the cell just picked is
    remembered, so its charge and requeue look nothing up;
  - a charge multiplies by each weight's reciprocal, computed when the
    weight is set (the CPU's weight sum: when it changes), never divides;
  - a domain's virtual time is 64 bits (2^-16 ns per unit of weight,
    compared by wrapping difference: unambiguous while the compared times
    are within 2^63 units, 39 hours of one CPU's time at weight 1); a
    cell's stays 128 bits, in the heaps only;
  - a domain leaves a CPU's list when a pick finds its heap there empty,
    not when its last cell is taken: a domain whose cell runs and comes
    back neither leaves nor joins;
  - every change of the tables marks them dirty, and the next locked
    section learns the queues again (O(entries), on tree changes only).
- **Cost of sharing by weight.** `cell.switch_domains_insns` (gated with
  `-icount`, ±2 %) is the switch with the two cells in two domains and a
  third, waiting cell placed under the first: 1,374 instructions on
  x86_64 and 1,285 on AArch64. The same three cells in round robin take
  887 and 829, so sharing by weight costs 487 and 456 instructions per
  switch (it was 1,250 and 1,070 with ordered maps, a list built at each
  pick and 64-bit divisions). Of those, measured with callgrind on the
  library alone (x86_64, `lto = "fat"` as the kernel builds, 105
  instructions per switch in round robin, 415 by weight):
  - taking a cell, about 150: the pick scans the two domains with work
    (about 24 each, with bounds checks), pops a heap, and the queue is
    searched by arrival;
  - putting it back, about 160: three charges (the cell's, its domain's
    and the CPU's virtual time: a 64-by-64 multiplication each), the cap
    check, the heap push and the list check, and the tests of the dirty
    mark and of a cap holding the cell.
  The kernel adds about 150: the clock read that measures the slice
  (`now_ns`, a 128-bit scaling of the cycle counter) and its own code
  generation of the same path. That is over the 200 instructions aimed
  at. The floor of this design is the work above: a domain choice, a
  heap per domain and three charges per switch, kept in step with a
  queue the round robin path also uses. Going lower would store the
  entries in the heaps themselves while sharing by weight (no parallel
  queue to search) and keep a heap of one cell inline; both change every
  operation of the scheduler, not only the pick, and are left as the
  next step if the figure matters more than the change's size.
- **Shares are computed when the tree changes.** The host recomputes every
  cell's domain, flattened share and capped nodes (`fair::flatten`) after
  a spawn, a copy, a resume, a placement and each settled end, only while
  it shares by weight; virtual times and cap use carry over. That is
  O(cells) per tree change in that mode.
- **Slices and caps.** The kernel charges the time from taking a cell to
  giving it back (a gang: from the last CPU that joined it, so a gang's
  slice counts once, not once per CPU). A cap is checked at the end of a
  slice, and a slice lasts up to two timer ticks (a cell is preempted at
  its second): a capped subtree overshoots by up to one such slice per
  period for each CPU running it.
- **Tree operations.** `Host::place` (protocol version 4: `run`, `fork`
  and `load` with `--cpu-weight` and `--cpu-max`, and `adopt`) and a root
  loaded from a snapshot share the CPUs again when the placed node is not
  plain or cells already share by weight: placing a plain root tests one
  field. `reparent` always does, and `set_limits_from` when cells share by
  weight. In-kernel tests and benchmarks place cells with `set_limits`,
  `adopt` and `place`, the weight or cap in the `Spec`
  (`root_weights_share_a_cpu`: roots of weights 300 and 100 get three
  quarters and a quarter of a CPU). `--cpus` (exclusive CPU sets) is
  enforced by task 1.9a (D17, "As built in phase 1").
- `sched.domain_share_pct`: 49 under `-icount` on AArch64.
- **Within one slice, for any number of domains (change
  `fix-fair-bound`).** A domain's finishing virtual time counts the
  longest slice charged so far, not the nominal 10 ms. With the nominal
  slice and slices a busy host stretched, three or more domains could
  stray up to about two of the longest slices. With the longest slice,
  each busy domain stays within one slice (the longest yet) of its
  weight's part, whatever the slices' lengths. That change's design
  (D1) gives the proof. In-kernel tests compare domains by their exact
  charged time, counted only in test builds.

Alternatives:

- One scheduling entity per tree level, as Linux's group scheduling.
  Rejected: its cost on every switch grows with the depth.
- Fairness per cell, as today. Rejected: cells would buy CPU.

## D24. Accelerators as capabilities

Rhizo must not be CPU-only. The accelerator architecture is the change
`add-accelerators`: a broker on the host owns every device and every
hardware partition, binds each to one domain at a time, and runs the
engines; cells never touch a device or map its buffers, and reach it
only through sessions the kernel relays (D1, D3 to D5 there). This
design only places that handle in the capability model, so that an
accelerator is held like any other resource.

**An accelerator handle.** It names an accelerator the host registered
with the broker, and carries the rights of `add-accelerators` D5:

| Object | Rights |
|---|---|
| Accelerator | infer (model sessions on the models the handle lists), compute (compute sessions, phase E there), stats (its subtree's use) |

- No cell holds one by default (no ambient authority). The host grants a
  root one (`rhizo run --accel gpu0:infer`).
- **Narrowing** is the hierarchy's generic derivation (D9): a derived
  handle has fewer rights, fewer models or a smaller budget, never more.
  It moves only over channels with the carry right, and its grantor can
  revoke it; a revoked handle opens no session, and open sessions end as
  `add-accelerators` specifies.
- **Partitions are not cell objects.** Devices and their hardware
  partitions (MIG instances, compute and memory partitions, virtual
  functions) are registered by the host and bound to a domain by the
  broker. No handle names one, and no right splits one.
- A child gets an accelerator handle only by derivation from its
  creator's own, through a grant or a `spawn` request; a module child
  only from its agent's (`add-cell-modules`).

**Budgets.** The accelerator budgets of `add-accelerators` D5 are limits
like memory and CPU time (D4), reserved only: device memory
(`accel.mem`), KV bytes in host memory (`accel.kv_host`, ordinary frames
charged to the cell's memory), device time (`accel.time`) and open
sessions (`accel.sessions`). A child's are carved from its parent's and
come back when it ends; a root's are admitted against what the broker
bound to its domain.

The budgets live in the handle, with the models it lists (one bit per
model the host registered for the accelerator, so at most 64): a child
gets its budget by derivation, carved from what its source handle has
free (its budget less what was derived from it and what its own
sessions use), and gives it back when the derived handle is revoked or
its holder ends. A derivation past what is free fails with `EDQUOT`, as
any request beyond a budget (`add-accelerators`); one asking for a right
or a model the source lacks fails with `EACCES`. The host's own grants
of an accelerator are admitted against the capacity it registered for
it.

**Exposure ceilings.**

- An accelerator shared by cells of a domain is a flow edge between them
  (D8): an engine that batches their sessions is one path.
- A device or partition serves at most one domain at a time; a device
  the hardware does not partition stays in one domain. The broker
  enforces it (`EBUSY`); the operator's override is audited and listed
  as a covert channel, like a missing CPU set (D17).
- KV and prompt caches carry the domain in their key (D21).

**Fork and snapshots.** A copy does not inherit the accelerator handle:
it opens no new session unless one is granted to it. The sessions the
source holds are channel endpoints, which the broker forks as
`add-accelerators` D6 specifies, charged to the copy's carved budgets. A
snapshot records the handle by label, as D13 records other handles, and
the host binds it again at load.

**Phase.** The handle kind, its rights, its narrowing, its budgets and
the exposure check come with phase 2 (handles), as plain data in
`rhizo-cap`, tested on the host. Sessions and the broker come with
`add-accelerators`, starting with its fake accelerator.
