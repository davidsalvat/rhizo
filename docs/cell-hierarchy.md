# Cell hierarchy, capabilities and containment

This is a readable summary of two proposed changes:

- [`add-cell-hierarchy`](../openspec/changes/add-cell-hierarchy/): the
  tree, budgets, egress, handles, channels, views and covert channels;
- [`add-cell-modules`](../openspec/changes/add-cell-modules/): adding
  capabilities as child cells.

The decisions and the alternatives that lost are in each change's
`design.md`, and the requirements in its `specs/`. Nothing here is
implemented yet. The user approved the design with one rule: **design
for production, not just the proof of concept** (see
[Decisions](#decisions)).

## The problem

> Agents must be self-contained within their cell or their own hierarchy.
> We don't want agents talking to each other to work out how to reach the
> internet when they shouldn't, communicating through things like
> renaming files. Communication and sharing between agents must be
> possible but strictly safe.

Rhizo already isolates memory, files and registers. But today every cell
shares one loopback network, any cell can reach any address through the
network card, and a few kernel-wide resources leak information between
cells:

- free memory, reported by `sysinfo`;
- ports, taken in order from one counter;
- the page cache;
- `getpid`, which counts every cell ever created.

There is also no structure above the single cell.

## What we promise

The attacker is cooperating agents trying to collude across hierarchies,
and agents trying to reach network destinations they are not allowed to
reach.

1. **No authority without a grant.** A new cell holds nothing. Every
   channel, every cell handle, every shared folder and every way out to
   the network comes from its creator, the host or an explicit grant.
   Handles cannot be forged, and sockets cannot be handed to another cell.
2. **Egress is enforced per socket.** A socket keeps the policy of the
   cell that made it. A cell with no egress can never make the network
   card send a byte, whatever it tells anyone.
3. **No storage channels between domains.** Apart from elapsed time,
   nothing a cell observes depends on what another domain does.
4. **Timing channels are reduced, listed and measured, not closed.** On
   shared CPUs and caches they cannot be closed. They carry information
   slowly, never authority or reach. For more, use separate VMs or
   exclusive CPUs.

So two colluding agents in different domains can signal each other only
slowly, through timing. Together they can reach at most the union of
their own egress policies, never more.

## The tree

```text
host
├── agent A          (root: domain 1)   egress: model, 1 GiB, 8 cells
│   ├── tools        egress: none, 256 MiB   ── channel ── A
│   └── python       (module)  egress: none
└── agent B          (root: domain 2)   egress: allowlist
```

- The host is the root. Each child of the host roots a **domain**.
- A cell created at a cell's request is its child.
- **Authority flows down.** A cell can suspend, fork, resume or kill only
  its descendants, and never act on its parent.
- **Re-parenting and promotion to root** are done by the parent, an
  ancestor or the host, never by the child. A control handle is never
  given to the cell it controls.
- **When a parent ends**, its children end with it (`cascade`, the
  default) or become roots (`promote`), as their creator chose.

## Budgets: no fork bombs

A child's memory, CPU time, sockets and number of descendants are carved
from its parent's budget when it is created, and come back when it ends.

- A subtree can never hold more than its root was given.
- Children are reserved, only: siblings never compete, so one cannot
  tell what the other uses. There are no shared pools. The budget record
  keeps a mode field so pools could be added later, with their own
  analysis.
- A root with a memory limit is reserved. The kernel admits roots against
  what it can really give, so one domain's use never shows in another's
  errors.
- File data and kernel records now count against the memory limit too.

## Networks and egress

- **Loopback becomes private per cell, and is never shared.** 127.0.0.1 in
  one cell is not 127.0.0.1 in another. That includes siblings and fork
  copies: cells talk to each other only through channels. A TCP tool
  server in another cell is reached through a small relay that carries
  TCP over a channel.
- **The network card is behind address translation in the kernel.** The
  card ports are random and no cell sees them, so ports cannot signal.
- **Egress policy per cell:**
  - `none`;
  - `model`, the model endpoint the host names;
  - an allowlist of address and port rules;
  - `full`.

  The policy is enforced in the kernel's sockets, inherited down the tree
  and only narrowed. **The default is `none`**: `rhizo chat` and agent
  templates pass `--egress model` explicitly. Denied connections get
  `EACCES` and are recorded in the audit log. Inbound datagrams from
  outside the policy are dropped.
- **A deny floor under every policy**, `full` included. By default it
  holds the host's own addresses as the guest sees them. On QEMU's user
  network the gateway leads to the host's 127.0.0.1, so no cell can
  reach a database or development server on the host. The only
  exceptions are the model endpoint the host grants, and the package
  proxy for builder cells.
- **The kernel knows addresses, not names.**
  - Allowing a DNS resolver allows exfiltration through DNS names.
  - An address shared by a CDN reaches every site behind it.
  - For name-level rules, give cells only the address of a host-side
    proxy.

## Handles and channels

| Object | Rights |
|---|---|
| A cell | observe, input, message, control |
| A channel endpoint | send, receive, carry |
| A view | read, write |

- Rights can only be reduced (attenuation).
- Handles move only over channels.
- The grantor can revoke what it granted, together with everything
  derived from it.

**Channels** are kernel message queues: bounded, copied by the kernel,
with no shared memory. To an unmodified Linux program a channel is a
connected `AF_UNIX` `SOCK_SEQPACKET` socket, so `read`, `write`, `poll`,
`epoll` and `SCM_RIGHTS` work as on Linux. A program finds its channels
in `/run/rhizo/`, a read-only directory in its own files:

```python
import socket
s = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
s.connect("/run/rhizo/chan/tools")
s.send(b'{"op": "search", "q": "..."}')
print(s.recv(65536))
```

**Exposure ceilings stop proxying.** The kernel refuses a grant that
would let a cell's information reach a cell with more egress than that
cell is allowed. A `none` agent cannot be given a channel to a `full`
agent unless its parent deliberately raises its ceiling. This is the
"talk to each other to reach the internet" case, closed by the kernel
rather than by advice.

## Shared files

- A **snapshot view** gives another cell a read-only, copy-on-write copy
  of a directory as it is now. It is cheap, and nothing flows afterwards.
- A **live view** shares a directory, read-only or read-write, between its
  owner and the cells it was granted to.
- A view cannot be escaped: `..` stops at its root and renames across its
  edge fail.

**Renaming files cannot signal.** Each cell has its own file tree. There
is no shared namespace except the views someone granted on purpose.

## The covert-channel inventory

The design lists 40 shared resources, each with what it can leak and what
is done about it. In short:

| Closed (no longer shared, or partitioned) | Accepted and measured (timing) |
|---|---|
| Loopback network and ports, card ports, socket pool, free memory (`sysinfo`), kernel heap and file data, page cache (partitioned per domain), template frames (held while shared), cell identifiers, the cell table, error codes | CPU contention and scheduling, hardware caches and TLB, 2 MiB block availability, the card's queues and neighbour cache, kernel locks |

The phase 1 benchmarks measure a CPU-contention channel between two
domains (`covert.cpu_bps`), and `docs/security.md` will publish the
inventory with that number.

## Exclusive CPU sets (production option)

The host can give a domain CPUs of its own (`rhizo run --cpus 2-3`). Its
cells run only there and no other cell does. That takes the scheduler and
the core-private caches out of the timing channels.

- **Not the default.** It costs density: the CPUs stay idle when their
  domain is idle, and work is never stolen across the set's edge.
- **Rules.** The boot CPU and at least one other CPU stay shared.
- **What it leaves shared:** the last-level cache, memory bandwidth, the
  kernel's locks and the card.
- **Host side.** For it to mean anything, the VMM must pin each vCPU to
  its own physical core (`rhizo up --pin`).
- **Measured.** `covert.cpu_bps` is measured with and without sets.

## Audit log

The kernel reports every grant, revocation, re-parent, promotion, policy
or limit change, cell creation and end, refused tree request and egress
denial to the host.

- **On the host.** `rhizo` appends the records to
  `~/.rhizo/audit/` (or `/var/log/rhizo/`) as JSON lines, each carrying
  the hash of the one before, so `rhizo audit verify` detects edited,
  removed or missing records.
- **Never blocking.** Cells never wait for the log. A full kernel buffer
  (64 KiB) drops records, counts them and reports the count. Repeated
  egress denials are folded to one record per cell and second.
- **Forwarding.** Production deployments forward the files to an
  append-only store.

## Limits and failure modes

Every structure has a bound, for example:

- tree depth 16;
- 1024 children per cell;
- 256 handles and 128 channels per cell;
- grant chains of 16;
- 64 rules per policy;
- 64-byte labels.

Exceeding one gives a clear error (`EMLINK`, `EMFILE`, `EINVAL`, `EDQUOT`)
and changes nothing. Tree operations are atomic: a fork into four that
fails after two undoes the two. If the host's link is lost, cells keep
running, and requests that wait for the host are refused at their
deadline.

## Upgrades

- **Snapshots.** The kernel reads the current snapshot format and the one
  before it. `rhizo snapshot upgrade` converts older files one version at
  a time, filling new fields with their safest values (egress `none`, no
  handles).
- **Protocol.** `rhizo` speaks the current protocol version and the one
  before it, and refuses older kernels with the upgrade steps:
  1. suspend and save;
  2. `rhizo down`, `rhizo up` with the new kernel;
  3. upgrade the files;
  4. load.

## Snapshots, forks and other machines

- The tree position, budget, egress, ceiling and handles are cell state
  and go into snapshots (format versions 11 to 13).
- A **fork** by the host or an ancestor makes siblings with the same
  policy and their budget carved again. The copies inherit derived
  handles. Channels to cells outside the copy read as reset, and the
  copies have no children.
- A **snapshot loaded from a file** carries no authority. The host places
  it. Its egress is narrowed to its new parent's and its limits capped.
  Its handles are dropped unless the host binds them by name
  (`rhizo load file --bind tools=12`).

## Commands

```text
rhizo run python --parent 3 --egress model --mem 256M --cells 4
rhizo ps --tree
rhizo reparent 7 --to root
rhizo policy 7 --egress allow:tcp:10.0.2.2/32:8080
rhizo grant channel 3 7 --label tools
rhizo grant view 3:/data 7:/mnt/data --snapshot
rhizo revoke G
rhizo scope 3 --socket ./agent3.sock   # a host socket limited to 3's subtree
rhizo kill 3 --tree
rhizo run agent --egress model --cpus 2-3 # an exclusive CPU set
rhizo audit tail --cell 7                # who granted what, what was denied
rhizo snapshot upgrade old.snap new.snap
```

A cell creates children (phase 4) through `/run/rhizo/kernel`, a channel
to the kernel granted with rights (`spawn`, `channel`, `derive`,
`revoke`, `narrow`).

## Modules: capabilities as child cells

Adding Python, numpy or Node.js to an agent means forking a child cell
from a warm template and talking to it over a channel. The module's
memory is never merged into the agent.

```text
agent ── add numpy ──► kernel: on the whitelist? within budget? egress?
                        └─ sensitive? ask the parent or the operator
agent ◄── /run/rhizo/chan/np ── child cell (copy of python+numpy template)
```

- **Templates form a tree** (`python` → `python+numpy` → `+pandas`) that
  shares memory copy-on-write, in memory and on disk (delta files).
- **File-only modules** are read-only views.
- **The agent adds modules itself**, within a whitelist and a budget set
  when it was created. Sensitive modules, and anything that would give
  the child egress, wait for the parent's or the operator's approval.
  A module never gets more egress than the agent: **adding `requests`
  grants no internet access.**
- **A root agent's sensitive modules always wait for the operator.** No
  policy can make them automatic.
- **A module child starts with nothing**: an empty module policy and no
  `spawn` right, so an `add` it sends is refused with `EACCES`. Modules
  run model-generated code and are the least trusted cells. When one
  needs another capability, it asks its agent over their channel. The
  agent, which sees the whole task, decides; if it agrees, it adds the
  module as its own child, a sibling of the requester, and may pass the
  requester an attenuated handle (message only). The tree stays flat and
  the budget stays whole. **Delegation is opt-in**: an orchestrator-style
  agent may give a module a subset of its whitelist and a slice of its
  module budget, and the module's adds become the agent's grandchildren.
  At every level budgets are carved from the parent, egress never exceeds
  the parent's, sensitive modules wait for the parent or the operator,
  and the depth limit of 16 holds. Each delegation is audited and shown
  by `rhizo ps --tree`.

  ```text
  default: the module asks its agent      opt-in: the agent delegates
  agent                                   agent
  ├── browser ─ "need documents" ─► agent └── coder   (delegated: python,
  │      ◄─ cell/docs, message only                    numpy; 2 children)
  └── docs     (added by the agent)           ├── python   (grandchild)
                                              └── numpy    (grandchild)
  ```
- **The catalogue holds only signed modules.** They are snapshot files
  plus a manifest, signed with minisign (Ed25519).
- **Key management.**
  - Secret keys stay with the operator: an encrypted key file, or an
    HSM or KMS behind a signing command. Cells, the kernel and builders
    never see them.
  - Trusted public keys carry a purpose and a validity.
  - `rhizo keys rotate` re-signs the catalogue and gives the old key a
    grace period.
  - Revocations go into a list signed by an offline revocation key, whose
    sequence number prevents rollback.
- **A builder cell is the only way into the catalogue.** It runs in its
  own domain. Its only egress is **Rhizo's own package proxy**
  (`rhizo proxy`) on the host, which:
  - serves only the files the build's lockfile names, from the
    configured index over verified TLS;
  - checks each file's pinned SHA-256 before serving a byte;
  - takes a token per build;
  - listens only where the guest reaches the host;
  - logs every request.

  The builder installs from that lockfile. Two builds
  must give the same file tree, and the result is signed after the
  operator reviews it. `pip install` inside an agent stays private to
  that agent's tree.
- **A small static chat agent**, the default of `rhizo chat` once it
  exists, talks to the model without CPython, and adds the `python`
  module only when the model asks it to run a snippet.

## Phases

| Phase | What | Key tests | Key benchmarks |
|---|---|---|---|
| 1 | Tree, reserved budgets, private loopback, address translation on the card, egress with the floor, storage channels closed, scoped host access, CPU sets, audit log, limits, upgrades | Differential colluders in two domains, egress per level, fork bomb, forged snapshot authority | No regression on system calls, switches, forks, loopback, network; `net.egress_check_insns`, `covert.cpu_bps` |
| 2 | Handles, channels, exposure ceilings, subtree snapshots | Forged handle numbers, rights cannot grow, sockets cannot be passed, proxy past the ceiling, revocation everywhere | `cell.channel_rtt_us` (not above loopback), `cell.handle_pass_us`, `cell.revoke_us`, `cell.grant_us` |
| 3 | Snapshot and live views | Renames do not signal, climbing out of a view, writing through read-only | `cell.view_grant_us`, `cell.view_read_us` |
| 4 | Cells creating cells | Fork bomb from inside, a child cannot escape | `cell.child_spawn_us` |
| Modules | Module policy, approvals, signed catalogue with key management, package proxy, builder, static chat | `requests` grants no internet, signatures, confined builder, private pip | `module.add_us`, `module.extra_bytes`, `chat.static_bytes` |

## Decisions

The user approved the recommendations, with the rule **design for
production, not just the proof of concept**:

1. **Pooled children:** not offered; reserved only, with room to add pools
   later.
2. **Default egress of a root:** `none`. `rhizo chat` and agent templates
   pass `--egress model`.
3. **The host's 127.0.0.1 through the gateway:** denied by every policy by
   default, except the model endpoint granted explicitly.
4. **Exclusive CPU sets per domain:** a supported production option, not
   the default, with its cost stated.
5. **Siblings and loopback:** no shared loopback; siblings talk only
   through channels.
6. **A root agent's sensitive modules:** always the operator's approval.
7. **The builder's package proxy:** Rhizo ships its own minimal proxy
   that enforces the lockfile's allowlist and checks pinned hashes.
8. **The static chat agent:** the default of `rhizo chat` once it exists.
9. **What a module child gets:** an empty module policy and no `spawn`
   right; it asks its agent, which adds a sibling. Delegation of a
   subset of the agent's whitelist and budget is opt-in.

The production requirements that came with these decisions are:

- key management for signing;
- the audit log;
- limits and failure modes;
- the upgrade path for snapshots and the protocol.
