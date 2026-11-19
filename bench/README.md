# Benchmarks

`cargo xtask bench` builds the release kernel and boots it repeatedly on every
virtual machine monitor available on the host (QEMU `microvm`, QEMU `q35`,
Firecracker, Cloud Hypervisor), next to a reference Linux guest booted the same
way. Firecracker and Cloud Hypervisor need KVM.

## Known issues

- The reference Linux guest is not measured on QEMU `microvm`: it resets right
  after KASLR setup, before `/init`, both with and without KVM. It is measured
  on QEMU `q35`, Firecracker and Cloud Hypervisor.

## Metrics

| Metric | Kind | Meaning |
|---|---|---|
| `boot.time_to_ready_ms/<guest>/<vmm>` | timing | Host wall clock from launching the VMM to the `rhizo-ready` line |
| `boot.kernel_cycles/rhizo/<vmm>` | timing | TSC cycles from the first kernel instruction to ready |
| `image.size_bytes` | deterministic | Size of the release kernel image |
| `memory.kernel_pt_frames` | deterministic | Frames used by the kernel's page tables |

Each timing is the median (and p90) of `--runs` boots after one discarded
warm-up boot. Lower is better for every metric.

## Environments and thresholds

`baseline.json` stores, per environment, the median of each Rhizo metric.
`cargo xtask bench --env <env> --check` fails when a metric is worse than its
baseline by more than its threshold:

| Environment | Where | Timing threshold | Deterministic threshold |
|---|---|---|---|
| `ci` | GitHub-hosted runners (shared, noisy) | +50 % | +1 % |
| `dedicated` | A self-hosted runner labelled `rhizo-bench`: bare metal or a pinned VM with KVM, fixed CPU frequency, nothing else running | +5 % | +1 % |

The `ci` gate only catches gross regressions; the `dedicated` run is the
precise one. It runs from the `bench-dedicated` workflow, started manually
(Actions → bench-dedicated → Run workflow) once such a runner is registered.

## Updating a baseline

A baseline changes only on purpose, in a pull request that explains why:

```bash
cargo xtask bench --env dedicated --update-baseline
```

CI publishes its measured results as an annotation named `bench-results` on
the `bench` job; to adopt them as the `ci` baseline, save that JSON to a file
and run:

```bash
cargo xtask bench --from results.json --env ci --update-baseline
```

## README chart

```bash
cargo xtask bench --readme
```

regenerates the chart in the README from a fresh measurement (or from stored
results with `--from`). Prefer numbers from the `dedicated` environment.
