---
name: benchmarks
description: Measure Rhizo's performance, prove an optimization with a benchmark, read the CI benchmark gate, and update baselines.
---

# Benchmarks

- Run: `cargo xtask bench` (release build, every available VMM, 20 boots
  each). Faster local loop: `--runs 5 --vmm qemu-microvm --no-reference`.
- Compare with the stored baseline: `--env ci --check`. Environments and
  thresholds are explained in `bench/README.md`.
- An optimization must come with a metric that shows it. Add the metric in
  `xtask/src/bench.rs` (deterministic metrics are best: counts, sizes), then
  show before/after numbers in the pull request.
- A regression needs a written justification in the pull request before
  `cargo xtask bench --env <env> --update-baseline`.
- Regenerate the README chart from measured results:
  `cargo xtask bench --from results.json --readme`.
- Timings without KVM are emulated and only orders of magnitude.
