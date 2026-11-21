# Tasks

## 1. Evidence

- [x] 1.1 Probe every free GitHub-hosted ARM runner for KVM (Linux), Hypervisor.framework under QEMU `hvf` (macOS, booting the AArch64 kernel) and virtualization extensions (Windows), with a temporary workflow; record the results and sources (D1); verify the probe runs' annotations
- [x] 1.2 Remove the probe workflow; verify `cargo xtask check` (workflow files)

## 2. Instruction counts on AArch64

- [x] 2.1 `bench --icount-only` (D2); host test `options_parse`; verify `cargo test -p xtask`
- [x] 2.2 Under `-icount`, clock figures complete the PMU passes (D3); host test `icount_figures_take_the_pmu_pass_then_the_clock`; verify two local runs of `cargo xtask --arch aarch64 bench --icount-only` agree exactly and include `cell.fork_insns`
- [x] 2.3 The AArch64 CI job runs it in `ci-arm` with `--check` and publishes `bench-aarch64-results-N`; the x86_64 job publishes compact JSON so its results fit its 39 pieces; verify the annotations of a CI run of this change
- [x] 2.4 `ci-arm` baseline from a CI run, after a second run agrees (D4); verify the next CI run passes the gate

## 3. README pipeline

- [x] 3.1 `--from` repeated merges results, a metric in two files is an error; `--readme` adds the instruction table when AArch64 figures are present (D5); host tests `results_of_both_architectures_merge`, `architectures_table_needs_aarch64_and_shows_both`; verify `cargo xtask check`
- [x] 3.2 `docs/img/gen.py` takes the AArch64 results as a second file and draws `arches.svg` (D5); verify it on a CI x86_64 run and a local AArch64 run, both themes rendered, no clipped or overlapping text
- [x] 3.3 Document it in `bench/README.md`; verify `openspec validate add-aarch64-bench --strict`
