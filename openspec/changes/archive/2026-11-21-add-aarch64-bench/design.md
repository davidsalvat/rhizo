# Design

## D1. No GitHub-hosted ARM runner accelerates an AArch64 guest

A temporary workflow (`probe-arm-accel.yml`, removed again in this
change) ran on every ARM label a public repository gets for free, on
2026-11-21 (runs 44855293209 and 44855941291 of this repository):

| Runner | What it is | Acceleration | Evidence |
|---|---|---|---|
| `ubuntu-22.04-arm`, `ubuntu-24.04-arm`, `ubuntu-26.04-arm` | Azure VM, Neoverse-N2 (Cobalt 100), kernels 6.8 and 7.0 `-azure` | none | no `/dev/kvm`; `dmesg`: `kvm [1]: HYP mode not available` (the guest kernel starts at EL1, so KVM cannot initialise) |
| `macos-15`, `macos-26` | Apple M1 (Virtual), `kern.hv_vmm_present: 1` | none | `kern.hv_support: 0` (macOS 26; the key is absent on 15); QEMU 11.1.3 `-accel hvf -cpu host` exits with `HV_UNSUPPORTED (0xfae9400f)` before running the kernel |
| `macos-14` | — | — | the label is no longer offered (`macos-14-arm64-us-default is not supported`) |
| `windows-11-arm` | Azure VM (`Model: Virtual Machine`), Cobalt 100 | none usable | `VirtualizationFirmwareEnabled=False`, `VMMonitorModeExtensions=False`; Hyper-V present only as the host's; the kernel's VMMs need KVM anyway |

The CI job's own report agrees on every run so far: "no /dev/kvm on
ubuntu-24.04-arm (no hardware virtualization); AArch64 runs are
emulated" (for example job 136685885928 of run 44852186988).

Sources for the runners nobody can probe for free:

- GitHub-hosted runners reference
  (docs.github.com/en/actions/reference/runners/github-hosted-runners):
  the free arm64 labels above; for arm64 macOS, "Nested-virtualization is
  not supported due to the limitation of Apple's Virtualization
  Framework".
- Larger runners
  (docs.github.com/en/actions/reference/runners/larger-runners): arm64
  Linux and Windows sizes, the M2 macOS XLarge with the same
  nested-virtualization limitation; and GitHub Actions billing
  (docs.github.com/en/billing/concepts/product-billing/github-actions):
  "Larger runners are billed for public repositories".
- Azure Dpsv6 series (learn.microsoft.com/azure/virtual-machines/sizes/
  general-purpose/dpsv6-series), the Cobalt 100 VMs these runners are:
  nested virtualization not supported.

So a free accelerated ARM runner does not exist, and a paid one would not
help either. Timings under TCG measure QEMU's translator, not Rhizo, and
move with the runner's load; they are not worth gating.

## D2. Instruction counts only (`--icount-only`)

Under `-icount shift=0,sleep=off` the guest clock advances one nanosecond
per instruction, so every figure the kernel times is a count of retired
instructions (spec `profiling`). That is exact under emulation and does
not depend on the host: the x86_64 job gates the same figures at +2 %.

| Option | AArch64 regressions caught | CI time | Noise |
|---|---|---|---|
| **`--icount-only` in the AArch64 job (chosen)** | every timed operation and breakdown, in instructions | about a minute (two release kernels, one run each) | none |
| The whole `bench` under TCG | the same, plus timings | tens of minutes (20 boots per VMM, workloads) | timings of QEMU, not Rhizo |
| Nothing (as before) | none | — | — |

`--icount-only` runs exactly what `bench` runs under `-icount` (the same
function), so both architectures measure the same way. It records no
`image.size_bytes`: a vmm-less metric would collide with x86_64's when
the results are merged (D4), and code size shows in the counts.

## D3. Clock figures complete the PMU passes

QEMU's AArch64 CPU has PMUv3, so the `bench-cell` kernel also runs its
counter passes under `-icount`, and those passes leave out the fork
benchmarks (they need memory the first pass keeps). Taking the PMU passes
alone, as before, dropped `cell.fork`, `cell.fork_touch` and
`cell.spawn_from_template` on AArch64. Now the clock gives every figure
and an exact PMU pass replaces it where there is one. The clock is
AArch64's generic timer, which under `-icount` ticks every 16
instructions: well within 2 % for operations of thousands of
instructions. Nothing changes on x86_64, where QEMU emulates no PMU.

## D4. Its own environment, gated from the first agreeing runs

`ci-arm` holds the AArch64 counts apart from `ci`: their VMM
(`qemu-virt-icount`) differs anyway, but a separate environment keeps
`--update-baseline` from one job from touching the other's figures, and
says in the baseline where the numbers come from. The baseline comes from
the first two CI runs of this change, which agreed exactly; QEMU's
version on the runner image is part of the measurement, so an image
update that changes counts shows as a gate failure to be justified like
any other baseline change.

## D5. Both architectures in the README pipeline

The AArch64 results are published under their own annotation titles
(`bench-aarch64-results-N`), so they are never mistaken for x86_64's.
`cargo xtask bench --from x86.json --from aarch64.json --readme` merges
them: the first file's environment, date and KVM label stay, and a
metric present in both files is an error rather than an arbitrary pick.
The README section adds "Both architectures, in instructions" only when
AArch64 figures are present, so a README built from x86_64 results alone
is unchanged. `docs/img/gen.py` takes the AArch64 file as an optional
second argument and draws `arches.svg` (light and dark) only then.
