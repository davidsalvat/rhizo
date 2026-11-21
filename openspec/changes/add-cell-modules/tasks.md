# Tasks

Nothing below starts before phases 1, 2 and 4 of `add-cell-hierarchy`
(and phase 3 for file-only modules) are in. The user decided the open
questions (proposal, "Decisions taken by the user").

## 0. Design

- [x] 0.1 Proposal, design and delta specs, and the modules section of `docs/cell-hierarchy.md`; verify with `openspec validate add-cell-modules --strict`
- [x] 0.2 The user's decisions recorded (operator approval for roots, Rhizo's own package proxy, static chat as default) with key management (D2, D4a, D5a, D6); verify with `openspec validate add-cell-modules --strict`

## 1. Modules and requests

- [ ] 1.1 Module policy (budget, entries, approver) in `rhizo-cap`, with subset and intersection (D3); verify `cargo test -p rhizo-cap module_policy` (property: a child's policy never exceeds its parent's)
- [ ] 1.2 Kernel module table; the `add` request on the kernel endpoint with the `modules` right; whitelist, catalogue, budget and egress checks; spawn and channel (D1, D2); verify the in-kernel `modules_are_child_cells` on both architectures
- [ ] 1.3 Approvals: the parent's `approvals` channel, host events, deadline, one pending request per cell, the operator always for roots, audit records (D2); verify the in-kernel `module_approvals` and `root_adds_need_the_operator` and the `control` test step (`rhizo approve`, `deny`)
- [ ] 1.4 `exec` and `repl:python` interfaces, `rhizo_module.py` and `rhizo-mod` (D1); verify the in-kernel `numpy_as_a_capability` with CPython built
- [ ] 1.5 File-only modules as snapshot views (D1); verify the in-kernel `file_only_module`
- [ ] 1.6 Snapshot format version 14: module policy, with its upgrade step from 13 and a recorded version-13 file (D7, H-D20); verify `cargo test -p rhizo-snapshot module_policy` and `version_13_file_upgrades`
- [ ] 1.7 Hostile tests: `requests_grants_no_internet`, `module_refusals`, `child_policy_is_a_subset`, `module_requests_fuzzed` (hostile frames on the endpoint); verify on both architectures

## 2. Signed catalogue

- [ ] 2.1 Module file format (snapshot, canonical manifest), minisign verification, trusted keys and revocation list in `cli` (D4); justify `ed25519-dalek` (BSD-3-Clause) and `blake2` (MIT or Apache-2.0) in `deny.toml`; verify `cargo test -p rhizo-cli module_files` (changed byte, unknown key, revoked digest)
- [ ] 2.1a Key management: `rhizo keys generate`, `rotate` (grace period), `revoke` (signed list, sequence number), trust directory with purpose and validity, permission checks, signing command (D4a); verify `cargo test -p rhizo-cli keys` (rotation, rollback refused, writable trust directory refused, signature outside validity refused)
- [ ] 2.2 `rhizo module load`, `list`, `check`, `unload [--kill]`, `resign`, `--unsigned`; verify the `control` test step
- [ ] 2.3 Delta files: base page records, base digest check, shared frames (D4); verify `cargo test -p rhizo-snapshot base_pages` and the in-kernel `delta_module_shares_its_base`

## 3. Builder

- [ ] 3.0 `rhizo proxy`: lockfile allowlist, upstream over verified TLS, hash check before serving, per-build tokens, limits, listening only where the guest reaches the host, JSON-lines log and build summary in the audit log, `--offline` from the cache; justify `ureq`, `rustls` and `sha2` in `deny.toml` (D5a); verify `cargo test -p rhizo-cli proxy` against a fake upstream (off-list project and version, wrong hash, foreign redirect, missing token, oversized file)
- [ ] 3.1 Builder template, `index` egress rule set (the proxy, carved out of the floor for builders only), lockfile install with hashes, tree digest, freeze (D5); verify the `control` step `builder_is_confined`, `builder_refuses_a_bad_hash` and `agents_cannot_reach_the_proxy`
- [ ] 3.2 `rhizo module build --twice`, `review`, `approve` (signing through minisign), `request`; verify the `control` test step (lockfile to signed catalogue file, digests equal)
- [ ] 3.3 Private `pip install` in an agent; verify the in-kernel `pip_inside_an_agent_stays_private`

## 4. Static chat agent

- [ ] 4.1 `cells/c/chat.c`, with `python` added at the first snippet; the default of `rhizo chat`, `--agent python` for CPython (D6); verify the `control` test step against the recorded transcript on both architectures

## 5. Benchmarks, documentation, CI

- [ ] 5.1 `module.add_us`, `module.extra_bytes`, `module.file_bytes`, `chat.static_bytes`, `chat.static_turn_us` (D8); verify `cargo xtask bench --env ci --check` with no gated regression
- [ ] 5.2 `docs/security.md`: modules, signing, builder; verify by reading
- [ ] 5.3 Everything CI runs; verify `cargo xtask ci` on both architectures and CI green
