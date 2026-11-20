# Tasks

## 1. Ports

- [x] 1.1 `Net::set_boot` picks where ephemeral ports start (D1); verify `cargo test -p rhizo-net each_boot_starts_its_ephemeral_ports_elsewhere` and `cargo xtask test --only ktest` on both architectures

## 2. Evidence and gate

- [ ] 2.1 CI benchmark: `workload.agent_turn_us` and `laptop.resume_turn_us` on Firecracker and Cloud Hypervisor in the same range as QEMU in every run (D2); verify the CI benchmark job's `rhizo-laptop` lines
- [ ] 2.2 Gate both turn metrics from two agreeing CI runs, `bench/README.md` updated (D2); verify `cargo xtask bench --from <run> --env ci --check` on both runs
