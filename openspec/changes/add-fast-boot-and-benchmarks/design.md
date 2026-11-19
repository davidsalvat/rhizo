# Design

## Context

See `proposal.md` for motivation and `specs/` for behavior. After H1 the kernel
boots on QEMU `q35` only (SeaBIOS plus a PVH option ROM), writes through a
polled 16550 UART, exits through `isa-debug-exit`, and `xtask` measures
nothing from the host side. CI runners are GitHub-hosted Ubuntu 24.04 machines
with KVM (enabled in the workflow), shared with other tenants and therefore
noisy.

## Goals / Non-Goals

**Goals:**

- Measure what a user waits for: VMM launch to kernel ready, on several VMMs.
- Make the default developer loop the fastest configuration.
- Catch performance regressions automatically without flaky CI.
- Plan the cell-primitive optimizations of later milestones with a benchmark
  for each one.

**Non-Goals:**

- Implementing cell primitives or their optimizations (H2–H6).
- Statistical rigor beyond medians and p90 over repeated runs.

## Decisions

### D1. VMM abstraction in `xtask`

A `Vmm` enum (`QemuMicrovm`, `QemuQ35`, `Firecracker`, `CloudHypervisor`)
knows how to launch a kernel image, which console it exposes, and how a run
ends:

| VMM | Launch | Console | End of run |
|---|---|---|---|
| QEMU `microvm` (default) | `-M microvm,rtc=off,pit=off,pic=off,isa-serial=on -kernel` (the option ROM stays: it implements PVH boot) | debugcon `0xE9` | `isa-debug-exit` status |
| QEMU `q35` | as today | debugcon `0xE9` | `isa-debug-exit` status |
| Firecracker | `--no-api --config-file` (JSON with `kernel_image_path`) | serial | reset via i8042 → process exits; result from console |
| Cloud Hypervisor | `--kernel --cpus boot=1 --memory size=128M --serial off --console off --debug-console tty` | debugcon `0xE9` | ACPI shutdown device (port `0x600`) → process exits; result from console |

`microvm` still runs QEMU's minimal `qboot` firmware for PVH (QEMU cannot boot
a PVH kernel with no firmware at all); Firecracker and Cloud Hypervisor enter
the PVH entry point directly with no firmware. `--vmm` and `--machine` flags
select the VMM; `--accel tcg` forces emulation.

Alternatives: one script per VMM (rejected: duplicated parsing and timing
logic); libvirt (rejected: heavy, slow, not available everywhere).

### D2. Console selection without probing cost

The kernel reads a one-byte console choice from the PVH command line
(`console=e9` or `console=serial`), which `xtask` passes per VMM. Default when
absent: serial (safe everywhere). Writes to `0xE9` are a single `out` per byte
with no status polling; the UART path stays as it is.

Alternative: probing `0xE9` by reading it back (QEMU returns `0xE9`). Rejected:
not all VMMs implement the read, and a wrong guess loses all output.

### D3. Ending a run on VMMs without an exit device

The `Power` implementation chooses per platform: `isa-debug-exit` on QEMU, an
i8042 reset (`out 0x64, 0xFE`) on Firecracker, which terminates the VMM process
on guest reset, and Cloud Hypervisor's ACPI shutdown device (port `0x600`,
S5 with the enable bit) because Cloud Hypervisor reboots the VM on a reset. Success or failure is then
decided by `xtask` from the console: `rhizo-ready` / `test result: ok.` versus
`PANIC` / `EXCEPTION`. The kernel learns the platform from the same command
line (`exit=debug-exit`, `exit=i8042` or `exit=ch-acpi`).

### D4. Ready marker and host timing

At the end of boot (and before running in-kernel tests) the kernel prints
`rhizo-ready cycles=<n>`. `xtask` records `Instant::now()` just before
spawning the VMM and timestamps each console chunk as it arrives; time to ready
is the timestamp of the chunk containing the marker. Reading uses a dedicated
thread with unbuffered pipes so the measurement adds microseconds, not
milliseconds.

### D5. Release profile

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
debug = false
```

Benchmarks always build `--release`. The debug profile stays for tests and
debugging.

### D6. 1 GiB direct-map pages

The kernel checks `CPUID.80000001h:EDX[26]` (Page1GB). With support, the
direct map uses 1 GiB pages (`PageSize::Huge` in `rhizo-core` paging, at the
PDPT level); without it, 2 MiB pages as today. The page-table code gains the
third size with host tests. Benchmark: page-table frames used and setup
cycles for the kernel address space.

### D7. Benchmarks, baseline and gates

- `cargo xtask bench [--runs N] [--vmm ...] [--output file] [--check]
  [--env ci|dedicated] [--readme]`, default 20 runs per VMM.
- Metrics: `boot.time_to_ready_ms` (host, per VMM), `boot.kernel_cycles`
  (guest, per VMM), `image.size_bytes`, `memory.kernel_pt_frames`.
- `bench/baseline.json`: `{ env: { metric/vmm: { value, threshold_pct } } }`.
  Medians are compared; a metric fails when it is worse by more than its
  threshold.
- Thresholds: deterministic metrics 1 %; `ci` timings 50 % (shared runners
  vary widely); `dedicated` timings 5 %.
- CI: a `bench` job runs `cargo xtask bench --env ci --check` on every pull
  request. A second workflow runs `--env dedicated --check` on a self-hosted
  runner labelled `rhizo-bench` (bare-metal or a pinned VM with KVM and CPU
  frequency fixed); it is triggered manually (a scheduled run would sit in
  the queue for a day whenever no such runner is registered).
- Updating a baseline is an explicit act: `cargo xtask bench --env <e>
  --update-baseline`, committed with a justification in the pull request.

Alternatives: comparing against the previous commit in the same CI job
(doubles CI time and still noisy); instruction counting with performance
counters (unavailable on most hosted runners).

### D8. Reference sandbox and README chart

The reference is a minimal Linux guest: Firecracker's published CI guest
kernel (pinned by version and SHA-256) with a tiny static init that prints
`rhizo-ready linux` and pauses; the host stops the VM once the line arrives (the
same "stop at ready" used for Rhizo in benchmarks). `xtask bench` boots it on
the same VMMs with the same timing method, so the chart compares like with
like on one machine. Each guest gets the smallest QEMU machine it needs: on
`microvm`, Linux keeps the legacy timers to calibrate its clock and needs
`earlyprintk=ttyS0` to register a console; under emulation it often resets
before `/init` on `microvm`, so that pair is only measured with KVM.
`--readme` rewrites the section between `<!-- bench:start -->` and
`<!-- bench:end -->` with a Mermaid `xychart-beta` bar chart (rendered by
GitHub), a table of the values, the environment label and the date. If the
reference artifacts cannot be downloaded, the chart shows Rhizo only and says
so.

### D9. Tools downloaded by `xtask`

Firecracker and Cloud Hypervisor static release binaries and the reference
guest kernel are downloaded on demand into `target/tools/`, pinned by version
and SHA-256. They are Apache-2.0 (VMMs) and GPL-2.0 (Linux guest kernel); all
are run as separate programs or guest images, never linked or vendored, so
Rhizo's MIT license is unaffected.

### D10. Optimization roadmap with benchmarks

Each planned optimization gets a named benchmark before it is implemented:

| Milestone | Optimization | Benchmark |
|---|---|---|
| This change | 1 GiB direct-map pages | `memory.kernel_pt_frames`, `boot.kernel_cycles` |
| H2 | 2 MiB pages for cell memory where possible | `cell.spawn_us`, `cell.pt_frames` |
| H3 | PCID: no TLB flush on cell switch | `cell.switch_ns` |
| H4 | Lazy restore: snapshot pages mapped on first touch (in-kernel analogue of Firecracker + `userfaultfd`) | `cell.resume_us`, `cell.resume_touch_us` |
| H5 | Lazy copy-on-write, including page tables, for `fork` | `cell.fork_us` (50 MiB cell), `cell.fork_pt_frames` |
| H5 | Pre-warmed cell templates | `cell.spawn_from_template_us` |

## Risks / Trade-offs

- [No KVM in some development environments] → Firecracker and Cloud
  Hypervisor are skipped with a clear message; QEMU falls back to TCG and
  timings are labelled as emulated.
- [Shared runners are noisy] → wide `ci` thresholds catch only gross
  regressions; precise gating needs the dedicated runner.
- [Reference artifacts unavailable (network policy, upstream moves)] → pinned
  checksums; the chart degrades to Rhizo-only with an explicit note.
- [Debug port output can be lost on a VMM without it] → explicit console choice
  per VMM (D2) instead of probing.
