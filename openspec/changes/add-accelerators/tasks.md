# Tasks

This change is designed first and holds no code yet. Its phases come
after `add-cell-hierarchy` phase 2 (handles and channels), in order, each
with its own tests (hostile ones included) and benchmarks. A phase that
is to be archived on its own is first split into its own change (design
D11).

## 0. Design

- [x] 0.1 Proposal, design (options, numbers, sources, decision table) and delta specs; verify with `openspec validate add-accelerators --strict`
- [ ] 0.2 The user's decisions on the open choices (recommended architecture, the cross-domain override, phase order); verify by recording them in the proposal and `openspec validate add-accelerators --strict`

## 1. Phase A: broker and model sessions (no GPU)

- [ ] 1.1 New crate `rhizo-accel`: frames with checked decoding, sessions, rights, budgets carved like `rhizo-cap`'s, the model fingerprint (D3, D4, D5); a row in `xtask/src/deps.rs` and in the code map of `AGENTS.md`; verify `cargo test -p rhizo-accel` (properties: decoding never panics on any bytes, budgets never exceed the parent's, rights never grow)
- [ ] 1.2 `AcceleratorLink` in `rhizo-hal` and `testing::FakeAccelerator` (toy model, deterministic KV bytes, prefill counter, device memory limit, injected failures) (D4); verify `cargo test -p rhizo-hal fake_accelerator`
- [ ] 1.3 The broker `rhizo accel` in `cli/` with the `fake` engine and the llama.cpp adapter, and `xtask` running it with a tiny GGUF model on the CPU (D3, D4); verify `cargo test -p rhizo-cli accel` and a host test against `llama-server`
- [ ] 1.4 The kernel's link over the card, accelerator handles with `infer` and `stats`, `/run/rhizo/accel/<label>` endpoints in `rhizo-abi`, budgets `accel.mem` and `accel.sessions` (D3, D5); verify `cargo test -p rhizo-abi accel` and the in-kernel `a_cell_chats_through_its_accelerator` and `no_handle_no_accelerator` on both architectures
- [ ] 1.5 Protocol version 5: `rhizo up --accel`, `rhizo accel ls`, `rhizo run --accel`, grants and revocations audited (D10); verify `cargo test -p rhizo-control` and the `control` test step on both architectures
- [ ] 1.6 Benchmarks `accel.token_rtt_us` and `accel.session_open_us` on QEMU, Firecracker and Cloud Hypervisor (D13); verify `cargo xtask bench` with no regression on CPU metrics

## 2. Phase B: fork, KV tiers, snapshots (no GPU)

- [ ] 2.1 The neutral KV block container and the content index keyed by domain (D7); verify `cargo test -p rhizo-accel kv` (properties: export then import gives the same tokens; no lookup crosses domains)
- [ ] 2.2 Session fork through the engine (llama.cpp sequence copy) and through export and import (D6); verify the in-kernel `forked_sessions_continue_without_prefill` with the fake and llama.cpp (prefill counter unchanged)
- [ ] 2.3 Tier 1: evicted KV written into the cell's frames, charged to `accel.kv_host`, shared within a domain, never across (D7, D9); verify the in-kernel `kv_frames_shared_within_a_domain` and `kv_frames_never_shared_across_domains`
- [ ] 2.4 Snapshot format version 15 with the accelerator section, version 14 read, loaded sessions bound only by the host (D10); verify `cargo test -p rhizo-snapshot` (`accel_section_round_trip`, `version_14_has_no_sessions`, `loaded_sessions_need_a_binding`, hostile properties)
- [ ] 2.5 Save and load across `rhizo down` and `rhizo up` without re-prefill, and re-prefill on a broker with another fingerprint (D6, D7); verify the `control` test step (`accel.resume_prefill_tokens` is 0)
- [ ] 2.6 Benchmarks `accel.session_fork_us`, `accel.resume_prefill_tokens`, `accel.kv_restore_ms_per_gib`, `accel.kv_shared_bytes` (D13); verify `cargo xtask bench`

## 3. Phase C: hosted APIs (no GPU)

- [ ] 3.1 The broker as the `model` gateway: API keys on the host only, sessions on hosted models, a fake API server with a prefix cache and lifetimes (D8); verify `cargo test -p rhizo-cli api_gateway`
- [ ] 3.2 Routing of fork families, breakpoints at fork points, lifetimes from the cell's state, cache keys derived per domain (D8, D9); verify `cargo test -p rhizo-accel routing` and the in-kernel `ten_forks_one_cache_write`
- [ ] 3.3 Benchmark `accel.api_cache_writes` (D13); verify `cargo xtask bench`

## 4. Phase D: GPUs (self-hosted runner)

- [ ] 4.1 A self-hosted GPU runner in CI (nightly), with its scope written in `docs/` (D11); verify a green nightly job
- [ ] 4.2 Engines: llama.cpp on CUDA, Vulkan and Metal; vLLM with LMCache; SGLang with HiCache (D7); verify each adapter's fork and resume test on the runner
- [ ] 4.3 IOMMU check, binding of devices and partitions (MIG, AMD CPX and NPS, SR-IOV VFs) to one domain, the audited override (D5, D9); verify `cargo test -p rhizo-accel binding` and the runner's `two_domains_never_share_a_device`
- [ ] 4.4 Scrub between domains, with a LeftoverLocals-style probe (D9); verify the runner's `leftover_memory_reads_zero_across_domains`
- [ ] 4.5 `accel.time` shares with deficit round robin, device time charged back (D5); verify `cargo test -p rhizo-accel time_shares` and the runner's share test
- [ ] 4.6 `covert.accel_bps` between two MIG instances; the covert-channel inventory in `docs/security.md` (D9, D13); verify by reading and the runner's measurement

## 5. Phase E: compute sessions (GPU)

- [ ] 5.1 Driver-API remoting library and executor, `compute` right, read-only weight sharing (D1, D6); verify PyTorch inference in a cell on the runner
- [ ] 5.2 Suspend and lazy fork through driver checkpoints (D6); verify the runner's `compute_session_survives_fork_and_resume` and `accel.compute_fork_ms`

## 6. Phase F: vsock and confidential GPUs

- [ ] 6.1 virtio-vsock in `rhizo-virtio` and the link over it (D3); verify `cargo test -p rhizo-virtio vsock` and the in-kernel chat test over vsock where the host has `vhost-vsock`
- [ ] 6.2 Confidential driver VM, attested link (D9); verify on H100 hardware in CC mode

## 7. Documentation and CI

- [ ] 7.1 `docs/vision.md` (GPU section), `docs/security.md`, `docs/audit-scope.md` (the broker as a new boundary), `README.md` per the archive rule; verify by reading
- [ ] 7.2 Everything CI runs, for each phase; verify `cargo xtask ci` on both architectures and CI green
