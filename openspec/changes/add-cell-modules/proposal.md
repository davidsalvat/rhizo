# Proposal

## Why

Agents need capabilities they did not start with: Python to run a
snippet, numpy or pandas for data, Node.js for a tool. The user decided how
to add them: **as connected child cells, never by merging memory into the
agent.** An agent adds a capability itself, within a whitelist and a
budget fixed when it was created. Sensitive capabilities need approval.
Shared modules come only from a signed catalogue, fed by a builder cell
that the operator approves.

The design uses the cell hierarchy (`add-cell-hierarchy`). Templates,
carved budgets, egress that only narrows, channels and views are already
there. This change adds the module-specific parts:

- the request flow;
- the whitelist;
- signing;
- the builder;
- a small chat agent that no longer carries CPython when it only talks.

## What Changes

- **Modules are templates with a manifest.** Adding one copies it as a
  child of the agent, connected by a channel (`exec` or `repl:<language>`
  interface, JSON messages). Sub-templates (`python` → `python+numpy` →
  `+pandas`) share memory copy-on-write. File-only modules are read-only
  snapshot views.
- **Request flow.** An `add` request goes to the agent's own kernel
  endpoint (the new `modules` right). The kernel checks the whitelist, the
  catalogue, the budget and egress, and spawns. Sensitive adds wait for
  approval by the parent (an `approvals` channel) or the host
  (`rhizo approve`).
- **Module policy** (TOML: a budget for all modules, carved from the
  agent's limits, and allow entries with version prefixes, caps, `ask`,
  `egress = "granted"`). It is cell state, inherited only as a subset.
  Adding a module never widens egress.
- **Signed catalogue.** Module files are snapshot files plus a canonical
  manifest, signed with minisign (Ed25519). Keys stay on the host; cells
  and the kernel never hold them. `rhizo module load` verifies
  signatures, the revocation list and the base digest. Delta files refer
  to their base module (snapshot format version 14).
- **Builder cell.** Its own domain. Egress is the package index only,
  through a host proxy. It installs from a lockfile with pinned versions
  and hashes, gives a reproducible file-tree digest (checked by two
  builds), and freezes. The host reviews, and signs after operator
  approval. Agents can request a module as a lockfile, never publish one.
  `pip install` inside an agent stays private to its tree.
- **Static chat agent** (`cells/c/chat.c`) that adds `python` only on its
  first snippet. `rhizo chat --agent static|python`.
- **`rhizo module` commands:**
  - `load`, `build [--twice]`, `review`, `approve`, `request`, `list`;
  - `rhizo approvals`, `approve`, `deny`;
  - `rhizo run --modules FILE`.
- **Tests:** add, refusals, approval paths, "`requests` grants no
  internet", signatures, builder egress and reproducibility, private pip,
  the static chat transcript.
- **Benchmarks:** `module.add_us`, `module.extra_bytes`,
  `module.file_bytes`, `chat.static_bytes`, `chat.static_turn_us`.

### Out of scope

- Changing a running cell's own memory or program (merging capabilities).
- Signature checks in the kernel.
- A public online catalogue or registry service.
- Node.js and other ecosystems' builders beyond the same pattern (the
  builder design is generic, and the first one built is Python's).
- Automatic updates of modules.

### Dependencies

This change needs phases 1, 2 and 4 of `add-cell-hierarchy` (tree,
budgets, egress, channels, the kernel endpoint) and phase 3 for file-only
modules, plus `snapshots-on-disk`. Its tasks start after those.

### Open questions for the user

1. **Who approves a root agent's adds.** Should the approver of a root
   agent's `ask` modules default to the host (an operator prompt), with
   no way to approve automatically by policy?
2. **The package proxy.** Should Rhizo ship the host-side package proxy
   for the builder, or document using an existing one (a pip-compatible
   mirror)?
3. **Defaults for `rhizo chat`.** Is the static chat agent the intended
   default for `rhizo chat` once it passes the transcript?

## Capabilities

### New Capabilities

- `cell-modules`: modules, the module policy, the request and approval
  flow, the signed catalogue, the builder cell.

### Modified Capabilities

- `snapshots`: module policy and delta page records (format version 14).
- `control-plane`: approvals and module loading from the host.
- `workloads`: the static chat agent.
- `benchmarks`: module metrics.

## Impact

- `crates/rhizo-cap` (module policy, budget, approval state).
- `crates/rhizo-abi` (the `modules` right of the kernel endpoint).
- `crates/rhizo-snapshot` (version 14, delta records).
- `crates/rhizo-control` (approval events, module load).
- `kernel/src/cell.rs` and `kernel/src/control.rs` (module table, spawn,
  approvals).
- `cli/` (`rhizo module`, verification; new dependencies `ed25519-dalek`,
  BSD-3-Clause, and `blake2`, MIT or Apache-2.0).
- `cells/c/chat.c`, `cells/python/rhizo_module.py`, `rhizo-mod`.
- `xtask/src/bench.rs`, `docs/security.md`.
