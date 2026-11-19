# Tasks

## 1. Build and kernel

- [x] 1.1 Add the optimized `[profile.release]` (D5); verify `cargo xtask build --release` boots and `readelf` shows no debug sections
- [x] 1.2 Parse the PVH command line (`console=`, `exit=`) in `rhizo-core` with host tests; verify unknown or missing options fall back to serial and debug-exit
- [x] 1.3 Add the `0xE9` debug console and the reset-based exit path in `kernel/src/arch`, selected from the command line; verify boot output on QEMU with `console=e9`
- [x] 1.4 Print the `rhizo-ready cycles=<n>` marker at the end of boot and before in-kernel tests; verify the boot check in `xtask test` requires it
- [x] 1.5 Add 1 GiB pages to `rhizo-core` paging (host tests) and use them for the direct map when CPUID reports support; verify an in-kernel test inspects the direct-map translation size

## 2. VMMs and host timing

- [x] 2.1 Introduce the `Vmm` abstraction with QEMU `microvm` as default and `q35` selectable, KVM when usable; verify `cargo xtask test` passes on both machines
- [x] 2.2 Measure time to ready on the host and print it with the kernel cycles after each run; verify `cargo xtask run` prints both
- [x] 2.3 Download pinned Firecracker and Cloud Hypervisor binaries into `target/tools/` and boot the kernel on both; verify in CI (KVM) that both reach the ready marker, and locally that missing KVM gives a clear skip message

## 3. Benchmarks

- [x] 3.1 Implement `cargo xtask bench` (release build, N runs per VMM, median/p90, JSON output, deterministic metrics); verify output on the CI runner
- [x] 3.2 Add `bench/baseline.json` with `ci` and `dedicated` environments and `--check` / `--update-baseline`; verify `--check` fails on an artificially tightened threshold and passes otherwise
- [x] 3.3 Add the reference Linux guest (pinned kernel and static init) and `--readme` chart generation; verify the README section is regenerated with values, environment and date
- [x] 3.4 Add the CI `bench` job (`--env ci --check`) and the optional self-hosted `dedicated` workflow; verify the job passes on the pull request

## 4. Documentation and rules

- [x] 4.1 README: numeric targets against Firecracker, Unikraft and Hyperlight as hypotheses with sources, optimization roadmap per milestone with benchmarks, benchmark chart section; verify links and the chart render on GitHub
- [x] 4.2 AGENTS.md: optimizations must ship a benchmark; no pull request may worsen a metric without written justification; `bench/README.md` explains environments and baselines; verify `cargo xtask ci` passes

## Workflow follow-up

- Archive this change with `openspec archive add-fast-boot-and-benchmarks` after it is merged.
