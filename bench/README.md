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
| `cell.spawn_us/rhizo/<vmm>` | timing | Time to create the `hello` cell (tables, image, stack), measured inside the kernel over 101 runs per boot (`bench-cell` build) |
| `cell.run_us/rhizo/<vmm>` | timing | Median time from spawn to the cell's exit and teardown (every frame returned) |
| `cell.pt_frames` | deterministic | Page-table frames of the `hello` cell's own half |
| `cell.switch_ns/rhizo/<vmm>` | timing | Time per switch between two cells yielding to each other (system call in, scheduler, system call out), with address-space tags (ASID/PCID) |
| `cell.suspend_us/rhizo/<vmm>` | timing | Suspending the `ballast` cell (8 MiB written) into a snapshot, all its memory freed |
| `cell.resume_us/rhizo/<vmm>` | timing | Lazy resume of that snapshot until the cell can run (pages come back on first touch) |
| `cell.resume_eager_us/rhizo/<vmm>` | timing | Eager resume: every page restored before the cell runs (the baseline lazy resume is measured against) |
| `cell.resume_touch_us/rhizo/<vmm>` | timing | Lazy resume plus the cell reading every page and exiting |
| `cell.snapshot_bytes` | deterministic | Size of that snapshot |
| `cell.fork_us/rhizo/<vmm>` | timing | Forking a cell with 50 MiB written (frozen as a template): shares its page tables, copies nothing |
| `cell.fork_touch_us/rhizo/<vmm>` | timing | That fork plus the child writing every page (copy-on-write of all 50 MiB) until it exits |
| `cell.fork_pt_frames` | deterministic | Frames a fork allocates (the child's root only) |
| `cell.spawn_from_template_us/rhizo/<vmm>` | timing | Starting `hello` from a frozen template (a fork) and running it to its exit, against `cell.run_us` from the executable |
| `workload.cold_us/rhizo/<vmm>` | timing | A real static glibc program (`libc_warm`: prime sieve warm-up, then one answer) started from its executable and run to its exit |
| `workload.warm_us/rhizo/<vmm>` | timing | The same answer from a cell forked off a template frozen after the warm-up |
| `workload.python_cold_us/rhizo/<vmm>` | timing | MicroPython (static, pinned v1.24.1) answering one request (`cells/python/rhizo_demo.py`, imported from the cell's files) started from its executable: interpreter start, imports, warm-up, answer. Measured only where MicroPython is built (`RHIZO_MICROPYTHON_SRC`, set in CI) |
| `workload.python_warm_us/rhizo/<vmm>` | timing | The same answer from a cell forked off MicroPython frozen after its warm-up |
| `cell.switch_untagged_ns/rhizo/<vmm>` | timing | The same without tags (TLB flushed on every switch): the baseline the tags are measured against |
| `cell.file_write_read_us/rhizo/<vmm>` | timing | The `filer` cell creating a file, writing 1 MiB in 64 KiB writes and reading it back, spawn to exit |
| `cell.fork_with_files_us/rhizo/<vmm>` | timing | Forking a cell (frozen as a template) that holds a 16 MiB file: the file blocks are shared, not copied |
| `cell.idle_bytes` | deterministic | Memory each of 1000 idle cells costs (forks of a template of `idle`, each run into its sleep): what "thousands of mostly idle cells per node" costs |
| `cell.suspended_bytes` | deterministic | Snapshot size of one idle cell |
| `cell.thread_create_us/rhizo/<vmm>` | timing | A thread of the `threads` cell creating a thread (raw `clone`, as `pthread_create` does) that exits at once, and joining it (futex wait on its clear-tid word); measured by the cell with `clock_gettime`, robust figure over 105 runs |
| `cell.parallel_us/rhizo/<vmm>` | timing | With 4 vCPUs: sixteen CPU-bound cells (`spin 5`, four per CPU) run on every CPU, wall time per cell (`bench-cell` booted with 4 vCPUs) |
| `cell.scaling_pct/rhizo/<vmm>` | timing | That batch's time on every CPU, times the number of CPUs, over its time stepping on one CPU, in percent: 100 is perfect scaling (lower is better) |
| `cell.ipi_wake_us/rhizo/<vmm>` | timing | Handing work to an idle CPU: post it, wake the CPU with an inter-processor interrupt, see it done (robust figure over 105) |
| `cell.futex_wake_ns/rhizo/<vmm>` | timing | One futex hand-off between two threads of a cell (wake the other, wait; the other runs): half a ping-pong round trip, best of five rounds of 200 |

Figures the kernel repeats a hundred times or more (`cell.spawn_us`,
`cell.run_us`, `cell.spawn_from_template_us`, the two file metrics) are the
minimum over five consecutive rounds of each round's median, so one noisy
moment of the host does not decide them.

Each timing is the median (and p90) of `--runs` boots after one discarded
warm-up boot. Lower is better for every metric.

## Comparisons with existing mechanisms

Measured in the same `cargo xtask bench` run, on the same machine and with the
same clock as the cells (spec `comparisons`):

| Metric | What |
|---|---|
| `snapshot.restore_ms/linux/firecracker` | A Linux guest snapshotted through Firecracker's API (`Full`), restored by a new Firecracker process until the VM runs again: how Firecracker-based sandboxes resume and clone. First restore (cold page cache) dropped |
| `snapshot.restore_api_ms/linux/firecracker` | The `PUT /snapshot/load` request alone |
| `process.fork_us/linux-process/host` | `fork` of a host process holding 50 MiB written, child exits, parent waits (`bench/host/linux_process.c`) |
| `process.fork_call_us/linux-process/<host or firecracker>` | The parent's `fork()` call alone, holding 50 MiB written (pairs with `cell.fork_us`) |
| `process.fork_touch_us/linux-process/<host or firecracker>` | `fork`, the child writes every page of the 50 MiB and exits, the parent waits (pairs with `cell.fork_touch_us`) |
| `process.<metric>/linux-process/firecracker` | The same program as the `/init` of a Linux guest in Firecracker: the VM the cell figures come from, so both sides pay the same virtualization (spec `comparisons`) |
| `snapshot.restore_ms/criu/host` | `criu restore -d` of a dumped process holding 8 MiB written (`bench/host/criu_target.c`), as long as CRIU's log says it took (from its start to "Restore finished"). First restore dropped |
| `process.spawn_us/linux-process/host` | `posix_spawn` of the same static `hello` the cell benchmark runs, until it exits |
| `sandbox.run_ms/gvisor/runsc` | `runsc run` of a gVisor sandbox (systrap platform, no network) running the same static `hello`, until it exits. First run dropped |
| `snapshot.restore_ms/gvisor/runsc` | `runsc restore --detach` of a checkpointed sandbox running the idle cell program, until `runsc state` reports it running. First restore dropped |

Firecracker needs KVM for the guest's architecture, gVisor needs `runsc`
(the benchmark job installs a pinned release, checksum verified) and CRIU
needs `criu` and root (the benchmark job installs it); elsewhere each is
listed as skipped with the reason. Process baselines need a Linux host of the target
architecture. The README table puts each cell operation next to these figures;
a column not measured in that run shows "—", never a quoted number.

Rhizo's cell figures in that table are the ones measured with Rhizo itself
running in Firecracker, so both sides pay the same virtualization. GitHub's
runners are shared virtual machines (nested virtualization: every VM exit is
expensive), so publishable numbers come from the `dedicated` environment below.

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
