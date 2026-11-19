# Tasks

## 1. Measurements

- [x] 1.1 Linux process baselines (`bench/host/linux_process.c`, `process.fork_us`, `process.spawn_us`)
- [x] 1.2 Firecracker snapshot restore through the API socket (`snapshot.restore_ms`, `snapshot.restore_api_ms`); skipped with a reason where it cannot run
- [ ] 1.3 Verify both on the CI benchmark job (KVM)

## 2. Publication

- [x] 2.1 Comparison table in the README section, with unit tests
- [x] 2.2 CI publishes the complete results in numbered pieces
- [ ] 2.3 README regenerated from a CI run; roadmap (H7); bench/README (comparisons, dedicated machine)
