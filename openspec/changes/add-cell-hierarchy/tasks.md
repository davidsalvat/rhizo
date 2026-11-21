# Tasks

This change is designed first. The user approved the design and decided
its open questions (proposal, "Decisions taken by the user"). The phases come in
order, each with its own tests (hostile ones included) and benchmarks. If
a phase is to be archived on its own, it is first split into its own
change (design D16).

## 0. Design

- [x] 0.1 Proposal, design, delta specs and `docs/cell-hierarchy.md`; verify with `openspec validate add-cell-hierarchy --strict`
- [x] 0.2 The user's decisions recorded (no pools, egress `none` by default, host addresses in the floor, exclusive CPU sets as an option, no shared loopback) with the production additions (D17 to D20); verify with `openspec validate add-cell-hierarchy --strict`
- [x] 0.3 The state-of-the-art review's decisions recorded: content sharing bounded by the domain, reseeding after copies and templates without secrets, CPU weights and caps with their accounting cost, devices as capabilities (D21 to D24); verify with `openspec validate add-cell-hierarchy --strict`

## 1. Phase 1: tree, budgets, containment, egress

- [x] 1.1 New crate `rhizo-cap`, with the tree, budgets (reserved only, with a mode field), the limits of D19 as constants checked at compile time, `on_parent_end` and `Egress` with its order and intersection (D1, D3, D4, D7); a row in `xtask/src/deps.rs` and in the code map of `AGENTS.md`; verify `cargo test -p rhizo-cap` (properties: budgets never go negative or over the root, narrowing is transitive, a subtree never holds more than its root, every operation with an injected allocation failure leaves the tree unchanged)
- [ ] 1.2 File data, names, thread and socket records, queues and `smoltcp` buffers metered (D4); verify `cargo test -p rhizo-fs metered` and the in-kernel `files_count_in_the_memory_limit` on both architectures
- [ ] 1.3 Reserved roots admitted against memory, sockets and the cell table; caches and unlimited roots kept off unused reservations (D4); verify the in-kernel `a_reserved_root_keeps_its_memory` on both architectures
- [ ] 1.4 A loopback network per cell in `rhizo-net`, never shared: ports, listeners, datagrams, Unix names keyed by (network, name), ephemeral counter per network; copies bind their listeners again in their own network (D5); verify `cargo test -p rhizo-net networks` and the in-kernel `separate_loopback_networks` and `forked_servers_in_their_network` on both architectures
- [ ] 1.4a The `loopback` cell's `pair` mode (two threads); `cells_talk_over_loopback`, `loopback_connection_across_suspend_of_one_end`, the forked-servers test, `cell.loopback_rtt_us`, `cell.tcp_resume_us` and the density loads moved to it, with the redefinition justified (D5); verify the in-kernel tests on both architectures and `cargo xtask bench`
- [ ] 1.5 Card address translation with random, invisible card ports (D6); verify `cargo test -p rhizo-net card_ports` and the in-kernel `ports_of_other_domains_do_not_show` on both architectures
- [ ] 1.6 Egress checks on outside connect, datagrams out and in, resets on narrowing; egress `none` by default; the deny floor holding the host's addresses by default with the model endpoint carved out (D7); verify `cargo test -p rhizo-net floor` and `cargo test -p rhizo-net egress` and the in-kernel `egress_levels_against_the_echo_server` on QEMU, Firecracker and Cloud Hypervisor
- [x] 1.7 `sysinfo` per cell and `getpid` per domain (D12, items 6 and 19); verify `cargo test -p rhizo-abi sysinfo_per_cell` and the in-kernel `identifiers_are_per_domain`
- [ ] 1.8 Page cache keyed and bounded per domain; cross-domain templates held until their last copy ends (D12, items 9 and 12); verify the in-kernel `file_pages_kept_per_domain` and `template_dropped_under_two_domains` on both architectures
- [ ] 1.8a Content sharing bounded by the domain: the `public` flag of files in `rhizo-fs`, `PageCache` keyed by `(scope, block, length)` with public pages loaded at publication, frame records of snapshots limited to the domain and public objects, private copies before a move across domains, `publish` and `withdraw` (D21); verify `cargo test -p rhizo-abi page_cache_scopes` (property: no key of one domain ever returns a frame filled for another, public pages excepted), `cargo test -p rhizo-snapshot frames_stay_in_the_domain`, and the in-kernel `same_file_in_two_domains`, `public_library_always_hits` and `promotion_unshares` on both architectures
- [x] 1.8b Copies told to reseed: copy generation and `/run/rhizo/generation`, `MADV_WIPEONFORK` and `MADV_KEEPONFORK` over every kind of copy, a copy's `getpid` unlike its source's, the sticky bit and publication refused without attestation; the CPython and Node.js templates reseed after a copy (D22); verify `cargo test -p rhizo-abi wipeonfork`, and the in-kernel `copies_draw_different_random` (with CPython built and with `RHIZO_NODE`), `wipeonfork_reads_zero_in_copies`, `generation_changes_only_in_copies`, `copy_pid_differs` and `tainted_template_not_published` on both architectures
  Follow-ups of 1.8b, done with the task named:
  - 1.7: numbering `getpid` per domain keeps a copy's identifier unlike its source's (`copy_pid_differs`, plus a copy made a new root once 1.9 can place one)
  - 1.8a: `publish` asks `Host::may_publish` and passes the host's attestation
  - 1.9b: the attestation goes into the audit log
  - 1.10: the generation, the wipe ranges and the sticky bit move into the tree section of format version 11 (kept beside the snapshot in the kernel until then; a loaded file gets a random generation, which the stored one then replaces, and `loads_of_a_copied_template_see_new_generations` keeps holding)
  - 2.3, 2.5 and 3.2: receiving a message, a handle or a view sets the sticky bit (`Cell::exposed`)
  - 2.4: `/run/rhizo/generation` becomes a file of the `/run/rhizo` directory
- [ ] 1.9 Tree operations in the kernel: placement, re-parenting, promotion, cascade, subtree kill, suspend and resume, authority checks (D1 to D3); verify the in-kernel `tree_operations` and `cascade_and_promote` on both architectures
- [ ] 1.9a Exclusive CPU sets per domain in `rhizo-sched` and the kernel, `rhizo run --cpus`, `rhizo up --pin` (D17); verify `cargo test -p rhizo-sched exclusive_sets` and the in-kernel `exclusive_sets_hold` with 4 vCPUs
- [ ] 1.9b Audit records in the kernel (64 KiB ring, folded denials, drop counts) and the host's hash-chained log, `rhizo audit tail` and `verify` (D18); verify `cargo test -p rhizo-cli audit` (edited, removed and missing records detected) and the `control` test step
- [ ] 1.9c CPU weights and caps: two-level pick by virtual time in `rhizo-sched`, flattened weights and capped ancestors computed on tree changes, `--cpu-weight` and `--cpu-max` (D23); verify `cargo test -p rhizo-sched fair_shares` (properties in D23), the in-kernel `domains_share_by_weight` and `cpu_cap_holds` on both architectures, and `cargo xtask bench --env ci --check` with `sched.domain_share_pct` reported and no regression of `cell.switch_ns`, `cell.parallel_us` or `cell.scaling_pct`
- [ ] 1.10 Snapshot format version 11, tree section; loaded snapshots placed and narrowed by the host (D13); reading of version 10, `rhizo snapshot upgrade` with a recorded version-10 file (D13, D20); verify `cargo test -p rhizo-snapshot` (`tree_state_round_trip`, `forged_tree_state_is_capped`, `version_10_file_upgrades`, hostile properties on the sealed form)
- [ ] 1.11 Protocol version 4 (`run` options, `list` tree fields, `reparent`, `policy`, `limits`, `scoped`, `--tree` flags), `rhizo ps --tree`, `reparent`, `policy`, `scope`; `rhizo chat`, `rhizo run python-agent` and agent templates pass `--egress model`; `rhizo` speaks protocol 3 and 4 and refuses older kernels with the upgrade steps (D14, D20); verify `cargo test -p rhizo-control`, `cargo test -p rhizo-cli`, and the `control` test step on both architectures
- [ ] 1.12 Hostile collusion tests: `differential_colluders` (every probe of the isolation scenario, sender against receiver in two domains, outputs compared), `colluders_on_loopback`, `fork_bomb_in_a_subtree` and `forged_snapshot_authority`, with colluder cells in `cells/src/bin`; verify on both architectures
- [ ] 1.13 Benchmarks: `net.egress_check_insns`, `net.tcp_rtt_us` with 16 rules, `covert.cpu_bps` with and without exclusive CPU sets (not gated), `cell.parallel_us` with sets; justify any growth of `cell.idle_bytes` and `cell.suspended_bytes` (D16); verify `cargo xtask bench --env ci --check` with no regression on `cell.syscall_ns`, `cell.switch_ns`, `cell.fork_us`, `cell.spawn_from_template_us`, `cell.loopback_rtt_us`, `cell.pipe_rtt_ns`, `net.tcp_rtt_us`, `workload.dynamic_*` or the density figures
- [ ] 1.14 `docs/security.md`: threat model of colluding cells, the covert-channel inventory with `covert.cpu_bps`, O6 closed, content sharing bounded by the domain (D21), what copies keep of their template (D22); `docs/audit-scope.md`: B2 rewritten; verify by reading
- [ ] 1.15 Everything CI runs; verify `cargo xtask ci` on both architectures and CI green

## 2. Phase 2: handles and channels

- [ ] 2.1 Grant table, rights, derivation, revocation and the exposure check in `rhizo-cap` (D8, D9); verify `cargo test -p rhizo-cap` (properties: no request raises rights; revocation removes the whole derivation subtree; after any sequence of grants every cell's exposure is within its ceiling; control never flows up)
- [ ] 2.2 Channels: queues, bounds and room reserved at creation (D10); verify `cargo test -p rhizo-cap channels`
- [ ] 2.3 Endpoints as `SOCK_SEQPACKET` descriptors in `rhizo-abi`: read, write, `sendmsg` and `recvmsg` with `SCM_RIGHTS`, poll, epoll, non-blocking mode, wait queues, `EPERM` for other descriptors (D9, D10); verify `cargo test -p rhizo-abi channels`
- [ ] 2.4 `/run/rhizo` in `rhizo-fs`: `chan/`, `cell/`, `inbox`, read only, `unlink` drops (D9); verify `cargo test -p rhizo-fs rhizo_dir`
- [ ] 2.4a Accelerator handles as plain data in `rhizo-cap`: rights `infer`, `compute` and `stats`, narrowing by derivation (rights, models, budget), the budgets of `add-accelerators` D5 carved and admitted, the exposure check, no inheritance by copies (D24); verify `cargo test -p rhizo-cap accelerators` (properties: a derived handle never exceeds its source; budgets never go negative). Sessions and the broker come with `add-accelerators`
- [ ] 2.5 Cell handles: observe stream, input, message (inbox), control (D9); verify the in-kernel `cell_handles` on both architectures
- [ ] 2.6 Forks and snapshots with handles: format version 12 with its upgrade step from 11, keys, parked endpoints, subtree bundles, labels bound at load (D13); verify `cargo test -p rhizo-snapshot handles` and the in-kernel `handles_across_suspend_and_fork` on both architectures
- [ ] 2.7 Host grants: `grant`, `revoke`, `grants`, `load --bind`; verify `cargo test -p rhizo-control` and the `control` test step
- [ ] 2.8 Hostile tests: `forged_handle_numbers`, `rights_cannot_grow`, `socket_cannot_be_passed`, `proxy_past_the_ceiling_refused`, `revoked_everywhere`, `control_never_flows_up`, a CPython agent pair on unmodified `socket` code; verify on both architectures
- [ ] 2.8a A relay program in `cells/` turning a local TCP port into channel messages, and the density loads' cell pairs over channels (D5); verify the in-kernel `tcp_tool_server_behind_a_channel`
- [ ] 2.9 Benchmarks `cell.channel_rtt_us`, `cell.handle_pass_us`, `cell.revoke_us`, `cell.grant_us`; verify `cargo xtask bench --env ci --check`, with `cell.channel_rtt_us` not above the last two-cell `cell.loopback_rtt_us` and no phase 1 metric regressed
- [ ] 2.10 Everything CI runs; verify `cargo xtask ci` on both architectures and CI green

## 3. Phase 3: shared file views

- [ ] 3.1 Snapshot views: subtree cloned copy-on-write into a mount (D11); verify `cargo test -p rhizo-fs snapshot_view`
- [ ] 3.2 Volumes and live views, read-only and read-write; `..`, `EXDEV`, revocation with `EIO`, charging to the owner (D11); verify `cargo test -p rhizo-fs live_view` (with a property: no path reaches outside the volume)
- [ ] 3.3 Views in forks and snapshots, format version 13 with its upgrade step from 12, `save --with-views`; verify `cargo test -p rhizo-snapshot views` and the in-kernel `views_across_suspend_and_fork`
- [ ] 3.4 Hostile tests: `renames_do_not_signal`, `climb_out_of_a_view`, `write_through_read_only`; verify on both architectures
- [ ] 3.5 Benchmarks `cell.view_grant_us`, `cell.view_read_us`; verify `cargo xtask bench --env ci --check`, with no regression on `cell.file_write_read_us` or `cell.fork_with_files_us`
- [ ] 3.6 Everything CI runs; verify `cargo xtask ci` on both architectures and CI green

## 4. Phase 4: cells creating cells

- [ ] 4.1 Kernel endpoint with rights, requests in `rhizo-control` frames (D10, D14); verify `cargo test -p rhizo-control cell_requests` (hostile input properties)
- [ ] 4.2 `spawn` (a fork of the requester, a copy of a template it holds, a program from its own files), `channel`, `derive`, `revoke`, `narrow`, `reparent`, `promote` (D2, D13); verify the in-kernel `children_from_inside` on both architectures
- [ ] 4.3 Hostile tests: `fork_bomb_from_inside`, `child_cannot_escape` (wider egress, self-promotion, re-parenting itself or its parent, a control handle on itself), `image_cache_untouched_by_cells`; verify on both architectures
- [ ] 4.4 Benchmark `cell.child_spawn_us` against `control.fork_us`; verify `cargo xtask bench --env ci --check`
- [ ] 4.5 `docs/security.md` (new boundary: the kernel endpoint), README status; verify by reading
- [ ] 4.6 Everything CI runs; verify `cargo xtask ci` on both architectures and CI green
