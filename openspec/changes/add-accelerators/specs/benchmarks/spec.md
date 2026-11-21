# Spec Delta

## ADDED Requirements

### Requirement: Accelerator metrics
`cargo xtask bench` SHALL report `accel.token_rtt_us`,
`accel.session_open_us`, `accel.session_fork_us`,
`accel.resume_prefill_tokens`, `accel.kv_restore_ms_per_gib`,
`accel.kv_shared_bytes` and `accel.api_cache_writes` against the fake
broker and llama.cpp on the CPU, and the GPU metrics on hosts that have
one, skipped otherwise.

#### Scenario: Metrics without a GPU
- **WHEN** `cargo xtask bench` runs on a host with no GPU
- **THEN** the CPU-measurable accelerator metrics are present, the GPU ones are reported as skipped, and no gated CPU metric regressed
