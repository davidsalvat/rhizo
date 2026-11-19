# Benchmarks

`cargo xtask bench` builds the release kernel and boots it repeatedly on every
virtual machine monitor available on the host (QEMU `microvm`, QEMU `q35`,
Firecracker, Cloud Hypervisor), next to a reference Linux guest booted the same
way. Firecracker and Cloud Hypervisor need KVM.

## Known issues

- The arm64 reference kernel (Firecracker's CI build) has no PL011 driver,
  which is the UART of QEMU `virt` and Cloud Hypervisor, so on AArch64 the
  reference Linux guest is measured on Firecracker only. A reference kernel
  with both drivers would fix this.
- GitHub's ARM64 runners do not expose KVM, so AArch64 CI runs are emulated
  and AArch64 benchmarks need the `dedicated` environment below.
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
| `cell.spawn_us/rhizo/<vmm>` | timing | Median time to create the `hello` cell (tables, image, stack), measured inside the kernel over 101 runs per boot (`bench-cell` build) |
| `cell.run_us/rhizo/<vmm>` | timing | Median time from spawn to the cell's exit and teardown (every frame returned) |
| `cell.pt_frames` | deterministic | Page-table frames of the `hello` cell's own half |
| `cell.switch_ns/rhizo/<vmm>` | timing | Time per switch between two cells yielding to each other (system call in, scheduler, system call out), with address-space tags (ASID/PCID) |
| `cell.suspend_us/rhizo/<vmm>` | timing | Suspending the `ballast` cell (8 MiB written) into a snapshot, all its memory freed |
| `cell.resume_us/rhizo/<vmm>` | timing | Lazy resume of that snapshot until the cell can run (pages come back on first touch) |
| `cell.resume_eager_us/rhizo/<vmm>` | timing | Eager resume: every page restored before the cell runs (the baseline lazy resume is measured against) |
| `cell.resume_touch_us/rhizo/<vmm>` | timing | Lazy resume plus the cell reading every page and exiting |
| `cell.snapshot_bytes` | deterministic | Size of that snapshot |
| `cell.switch_untagged_ns/rhizo/<vmm>` | timing | The same without tags (TLB flushed on every switch): the baseline the tags are measured against |

Each timing is the median (and p90) of `--runs` boots after one discarded
warm-up boot. Lower is better for every metric.

## Environments and thresholds

`baseline.json` stores, per environment, the median of each Rhizo metric.
`cargo xtask bench --env <env> --check` fails when a metric is worse than its
baseline by more than its threshold:

| Environment | Where | Timing threshold | Deterministic threshold |
|---|---|---|---|
| `ci` | GitHub-hosted runners (shared, noisy) | +100 % on time to ready; TSC cycles recorded but not gated | +1 % |
| `dedicated` | A self-hosted runner labelled `rhizo-bench`: bare metal or a pinned VM with KVM, fixed CPU frequency, nothing else running | +5 % | +1 % |

Why so wide: two CI runs of identical code differed by up to +41 % in time to
ready and +53 % in kernel cycles. Hosted runners land on different physical
machines whose TSC frequencies differ, so cycle counts are not comparable
between runs there. The `ci` gate therefore only catches gross regressions
(for example, a boot that doubles); the `dedicated` run is the precise one. It runs from the `bench-dedicated` workflow, started manually
(Actions → bench-dedicated → Run workflow) once such a runner is registered.

## Measuring on real ARM64 hardware (`dedicated`)

Any ARM64 Linux machine with KVM works; the benchmarks only need QEMU,
Firecracker and Cloud Hypervisor to run guests with hardware acceleration.

| Machine | Notes |
|---|---|
| Raspberry Pi 5 (8 GB) | Cortex-A76; use a 64-bit OS with KVM enabled (Raspberry Pi OS 64-bit or Ubuntu Server). Cheapest option; thermal throttling: add a fan and fix the CPU governor |
| Orange Pi 5 / RK3588 boards | Cortex-A76 + A55: pin the benchmark to the big cores (`taskset -c 4-7`) |
| Cloud bare metal | AWS Graviton `*.metal` (e.g. `c7g.metal`), Ampere Altra servers (Oracle A1 bare metal, Hetzner RX): closest to production |

Setup, once:

```bash
sudo apt-get install -y qemu-system-arm
```

```bash
ls -l /dev/kvm
```

```bash
sudo cpupower frequency-set -g performance
```

Then register the machine as a self-hosted GitHub Actions runner with the
labels `self-hosted`, `rhizo-bench` and `arm64`, and start the
`bench-dedicated` workflow with `arch: aarch64`. The first run downloads the
AArch64 Firecracker and Cloud Hypervisor releases, which are not pinned yet:
check them, add their SHA-256 to `xtask/src/tools.rs` (the run prints them),
and commit the result before trusting the numbers. To try once before
pinning: `RHIZO_ALLOW_UNPINNED=1 cargo xtask bench`.

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
