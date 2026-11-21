# Proposal

## Why

The user's requirement, translated from their own words:

> Agents must be self-contained within their cell or their own hierarchy.
> We don't want what happened recently at some company: agents started
> talking to each other to work out how to reach the internet when they
> shouldn't, and ended up hacking Hugging Face, communicating through
> things like renaming files. Communication and sharing between agents
> must be possible but strictly safe.

Today Rhizo isolates memory, files and registers, but it has no structure
above the single cell, and it leaves direct and covert paths between cells
open:

- **Every cell shares one loopback network** (spec `networking`, "Loopback
  network between cells"). Any cell can reach any other cell's listening
  port. This is ambient authority.
- **The network card is shared with no egress policy.** Any cell reaches
  any address the VM reaches, the host's own loopback included through
  QEMU's user network.
- **Shared kernel state leaks between cells:**
  - `sysinfo` reports global free memory;
  - ports come from one sequential counter, and `EADDRINUSE` shows other
    cells' ports;
  - the page cache can be flushed by any cell and probed by timing;
  - `getpid` counts every cell ever created (O6);
  - file data and kernel records fall outside the memory quota.
- **There is no way to express "these agents may talk, those may not"**,
  no way for an agent to own helpers, and no quotas a parent can split
  among children. A cell cannot create a cell at all.

## What Changes

This change is the design of the cell hierarchy, capabilities and
containment, implemented in four phases (design D16). It holds no code.

**Phase 1: tree, budgets, containment, egress.**

- **Ownership tree**:
  - The host is the root, and each child of the host roots a domain.
  - Authority flows down only.
  - Re-parenting and promotion are authorized by the parent, an ancestor
    or the host, never by the child.
  - `on_parent_end` is `cascade` (the default) or `promote`.
- **Hierarchical budgets** (memory, CPU time, sockets, descendants):
  reserved in full from the parent's, so fork bombs are impossible. There
  are no pools, but a mode field leaves room for them. Reserved roots are
  admitted against what the kernel can give. File data and kernel records
  are charged to memory.
- **A loopback network per cell, never shared.** Siblings and copies talk
  only through channels. The two-cell loopback tests and benchmarks become
  two threads of one cell.
- **The card behind kernel address translation**, with random card ports
  invisible to cells.
- **Egress policy per cell** (`none`, `model`, an allowlist, `full`),
  enforced in `rhizo-net`, inherited and only narrowed. **The default is
  `none`.** `rhizo chat` and agent templates pass `--egress model`.
  Sockets never move between cells.
- **A deny floor under every policy.** By default it holds the host's own
  addresses as the guest sees them (QEMU's gateway to the host's
  127.0.0.1, the tap's host address), except the model endpoint granted
  explicitly.
- **Exclusive CPU sets per domain**: a supported production option, not
  the default, with its cost in density stated (D17).
- **Audit log on the host**, append-only and hash-chained: grants,
  revocations, re-parents, policy changes, egress denials (D18).
- **Limits and failure modes** for every structure (tree depth 16, 256
  handles per cell, and so on), with atomic tree operations (D19).
- **Upgrade path:** the kernel reads the previous snapshot version,
  `rhizo snapshot upgrade` converts older files, and `rhizo` speaks the
  previous protocol version (D20).
- **Storage channels between domains closed:**
  - `sysinfo` per cell;
  - `getpid` numbered within the domain;
  - the page cache partitioned per domain;
  - templates used by several domains pinned until their last copy ends;
  - socket, port and cell-table budgets admitted against the kernel's
    pools.
- **Covert-channel inventory** in `docs/security.md`, with a measured
  CPU timing-channel bandwidth.
- **Snapshot format version 11** (tree state). **Control protocol version
  4:**
  - `rhizo run --parent/--egress/--mem/...`;
  - `rhizo ps --tree`, `reparent`, `policy`;
  - subtree `kill`, `suspend` and `resume`;
  - host access scoped to a subtree (`rhizo scope`);
  - `rhizo audit`.

**Phase 2: handles and channels.**

- Handles with rights (observe, input, message, control on cells; send,
  receive, carry on channels).
- Attenuation, transfer over channels, and revocation by the grantor.
- No ambient authority.
- Kernel channels: bounded, copied, no shared memory. They appear as
  `AF_UNIX` `SOCK_SEQPACKET` descriptors under `/run/rhizo/`, carry
  handles as `SCM_RIGHTS`, and work with read, write, poll and epoll.
- **Exposure ceilings**: the kernel refuses any grant through which a
  cell's information could reach egress beyond its ceiling, so no
  proxying past a policy.
- Snapshot format version 12 (handles) and subtree snapshots.

**Phase 3: shared file views.** Snapshot views (copy-on-write, read-only)
and live views (volumes, read-only or read-write), granted by handle,
revocable, unable to escape their subtree. Snapshot format version 13.

**Phase 4: cells creating cells.** A kernel endpoint `/run/rhizo/kernel`,
granted with rights, takes `spawn` (a fork of the requester, a copy of a
template it holds, or a program from its own files), `channel`, `derive`,
`revoke`, `narrow`, `reparent` and `promote` requests for descendants.

**Tests** in every phase include hostile cells that try to collude across
domains. **Benchmarks** in every phase follow rule 7, and rule 8 for the
hot paths (design D16).

### Out of scope

- **Modular cells.** Designed in `add-cell-modules`, in the same pull
  request, built on this design (D15).
- **Closing timing channels on shared CPUs.** They are reduced, listed and
  measured (G4). Exclusive CPU sets remove the scheduler and core-private
  caches from them. Fixed time partitions per domain are not designed.
- **Name-based or content-based egress** (DNS, TLS SNI). The kernel
  filters addresses and ports. Names belong to a host-side egress proxy
  granted as an address.
- **Inbound connections from the network to cells.**
- **Signed snapshot files.**
- **Host shared folders.** Views leave room for them.

### Decisions taken by the user

The user approved the recommendations, with the rule: **design for
production, not just the proof of concept.**

1. **Pooled children:** not offered; children are reserved only. The
   budget record leaves room to add pools later.
2. **Default egress of a root:** `none`. `rhizo chat` and agent templates
   pass `--egress model` explicitly.
3. **The host's 127.0.0.1 through the gateway:** denied by every policy by
   default, except the model endpoint granted explicitly.
4. **Exclusive CPU sets per domain:** a supported production option, not
   the default, specified with its cost.
5. **Siblings and loopback:** no shared loopback; siblings talk only
   through channels.

The production requirements that came with them are the audit log, the
limits and failure modes, and the upgrade path. Module key management is
in `add-cell-modules`.

## Capabilities

### New Capabilities

- `cell-hierarchy`: the ownership tree, authority, budgets, handles,
  channels, exposure ceilings and cells creating cells.

### Modified Capabilities

- `isolation`: hierarchical quotas, everything charged, no storage channel
  between domains, pinned cross-domain frames, file pages per domain.
- `networking`: loopback per cell, egress policy, kernel-chosen card
  ports, sockets that stay with their cell, forks across networks.
- `snapshots`: format versions 11 to 13, tree state, handles, loaded
  snapshots without authority.
- `control-plane`: protocol version 4, tree requests, grants, scoped
  access.
- `fork`: where copies go and what they hold.
- `files`: views.
- `linux-abi`: identifiers per domain, `sysinfo` per cell.
- `benchmarks`: containment, channel, view and child-cell metrics.

## Impact

- **New leaf crate `crates/rhizo-cap`**: tree, budgets, egress policies,
  grant table, channel queues, exposure check. It has no dependencies and
  is added to `xtask/src/deps.rs` and to the code map in `AGENTS.md`.
- `crates/rhizo-net`: networks, policy checks, address translation.
- `crates/rhizo-cell`: quota to budget.
- `crates/rhizo-fs`: metering, `/run/rhizo`, mounts and volumes.
- `crates/rhizo-abi`: SEQPACKET endpoints, `SCM_RIGHTS`, `sysinfo`,
  `getpid`.
- `crates/rhizo-snapshot`: versions 11 to 13, reading the previous
  version, upgrade steps.
- `crates/rhizo-control`: version 4, the previous version on the host
  side, `audit` frames.
- `crates/rhizo-sched`: exclusive CPU sets.
- `kernel/src/cell.rs`, `kernel/src/control.rs`, `kernel/src/tests.rs`,
  `cells/src/bin` (hostile colluders, the `loopback` cell's thread pair),
  `cli/` (audit log, `snapshot upgrade`, floor, `--cpus`),
  `xtask/src/bench.rs`,
  `docs/security.md`, `docs/audit-scope.md` (B2 changes, a new boundary
  for the kernel endpoint).
- No new third-party dependency.
- No cost on system calls, switches or copy-on-write. Small, measured
  costs on fork, loopback, outside connects and file-page faults (D16).
