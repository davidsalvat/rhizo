# Design

## Evidence

All 18 completed `bench` runs on main from `b696efa` to `e655beb` (job
logs). The table printed by `cargo xtask bench` gives every median; a
`skipped pmu/qemu-microvm` line marks a guest without counters:

| Class | Runs (commit) | Boot cycles, microvm | `cell.syscall_ns` | Guest PMU |
|---|---|---|---|---|
| X | `b696efa`, `008a4c5`, `5a23f6d` | 4.35 to 4.36 M | 52, 53 | no |
| Y | `9810a4c`, `9ec9988`, `e24df84`, `ae7f3f7`, `79ef9a9`, `f182f0e`, `f5094a1`, `d211ad7`, `2ee4667`, `8f942c6`, `e655beb` | 6.35 to 6.77 M | 68 | yes |
| Z | `693be50`, `89c1cee`, `2a3beec` | 5.11 to 5.64 M | 62 to 64 | no |
| ? | `3e59e35` | 5.53 M | 87 | no |

`3e59e35` matches no class: cycles like Z, but a slower system call. It
may be a fourth class or a noisy Z run, and recorded model names will
tell.

Against the default `ci` baseline, a metric's median moves with the class
(median over the gated timings): X 0.88x, Y 1.00x, Z 1.10x. The spread is
uneven. On Z, 14 gated timings had a run above twice their baseline:
`cell.fork_with_files_us` on all four VMMs, the agent turns, the CPython
agent's model time, `laptop.resume_turn_us` and the control path to
first output on Cloud Hypervisor. No run on X or Y had any. Within a
class, the median spread (largest over smallest run) of a gated timing is
1.05 on X, 1.08 on Y and 1.04 on Z. The comparison counts each metric
only in runs made after its baseline last changed: the control and chat
figures changed with `008a4c5`, and the density p99s regressed for real on
`89c1cee` to `2ee4667` (fixed by `2a3beec`, `fix-tree-removal`).

The logs name no CPU model. GitHub prints the runner image but not the
machine, and nothing in the workflow printed `/proc/cpuinfo`. So the
model strings of X, Y and Z cannot be checked against history. This
change starts recording them.

## D1. The class key is the CPU model name

The class of a runner is its `/proc/cpuinfo` model name, normalized
(lower case, `(R)`/`(TM)` dropped, other characters one `-`). The
normalized form is readable and works as a JSON key and a command-line
argument.

The model name is the property of the hardware that the hosted runner
pools differ by. It needs no thresholds of its own, and it does not move
when the code changes. Family, model, stepping and microcode are recorded
but stay out of the key. Microcode is updated in place and would split one
pool into several classes. Family, model and stepping follow from the
model name.

Alternatives that lost:
- **Classes inferred from the figures** (boot cycles, `cell.syscall_ns`,
  PMU), as the analysis had to do. The thresholds between classes would
  be one more thing to tune. The figures are what the gate measures, so a
  regression of `cell.syscall_ns` could move a run into a slower class
  and pass. `3e59e35` already falls between two classes.
- **The TSC frequency.** It is recorded, but under nested virtualization
  it is what the hypervisor reports, and two CPU models can share it.

On AArch64, whose `/proc/cpuinfo` has no model name, the key comes from
the CPU implementer and part (`cpu-implementer-0x41-part-0xd4f`). No
AArch64 job measures timings in CI today.

## D2. Per-class baselines, timings only, and only the default's

`bench/baseline.json` gains a top-level `classes.<env>.<class>` object of
metric keys to medians, next to `ungated` and `thresholds_pct`.
`environments.<env>` stays a flat map of metrics, and every existing
reader of it is unchanged. For a run of class `c`, the values compared
are `environments.<env>`, with each timing that `classes.<env>.c` has put
in its place. Thresholds are the environment's, as today.

The rules make the classes hard to abuse:
- Only timings: deterministic metrics and `-icount` counts are the same on
  every machine, so they always come from the default. The gate reads only
  timings from a class, and it fails on any other entry, so a class cannot
  quietly hold a looser instruction count.
- Only metrics the default has: a class re-values gates, it never adds
  one. The default stays the list of what is gated (and of what `missing`
  reports when not measured).
- A class without a value for a metric uses the default's. A metric added
  later is gated on every class from its first default value.

Alternatives that lost:
- **A scale factor per class**, learned from history. It is one number per
  class, but the classes do not scale uniformly: on Z the noise metrics
  read 2.0 to 2.4x while the median timing reads 1.10x. A factor large
  enough for `cell.fork_with_files_us` would loosen every other timing by
  2x on that class and hide real regressions. A factor "learned" from
  recent runs would also creep up with regressions, which is an
  automatic baseline raise.
- **`environments.ci.classes.<class>`**, nested inside the environment.
  It mixes objects into a map of numbers that `missing`,
  `update_baseline` and the README tooling iterate.
- **A wider `ci` threshold.** It loosens every class.

## D3. Unknown classes gate against the default, loudly

A run whose class has no entry is gated against `environments.<env>`,
exactly as before this change. The gate prints `unknown runner class:
<model>; compared against the default class` (`not recorded` when the
results predate this change). With no `classes` at all, which is the
state this change leaves, every run is "unknown" and the gate behaves as
it does today. The line is printed only when the run measured a timing, so
`--icount-only` runs stay quiet.

## D4. New classes are explicit and never raised by a run

`--update-baseline --class <c>` exists only to write a new class's
entries from one results file without editing JSON by hand. It refuses
when:
- the results' own class is not `<c>`, which stops a run of one class
  from becoming another class's baselines;
- `classes.<env>.<c>` exists, so a class's values change only by a
  reviewed edit with a justification (rule 8);
- the environment has no default baseline.

It writes the run's timings that the default has, and nothing else. The
command prints that the change needs the owner's approval. `bench/README.md`
says the same. The gate never writes the baseline.

## D5. What is recorded, and from where

`runner` in `bench-results.json`:
`class`, `model_name`, `cpu_family`, `model`, `stepping`, `microcode`
(strings, as `/proc/cpuinfo` prints them), `cores` (its `processor`
entries), `kvm`, `guest_pmu` and `tsc_mhz`.

- `guest_pmu` comes from the results: `true` when a VMM not under
  `-icount` produced `_insns` counts (the PMU passes), `false` when a
  `pmu/<vmm>` skip says a guest had none, and `null` when no guest ran the
  cell benchmarks. It needs no new probe.
- `tsc_mhz` is the host kernel's "Refined TSC clocksource calibration",
  else its "tsc: Detected … MHz processor". The log is read with `dmesg`,
  or with `sudo -n dmesg` where `dmesg_restrict` hides it, as on Ubuntu
  runners. xtask already uses password-less `sudo` for tap devices. It is
  `null` when neither works. Under KVM the guest's TSC runs at the host's
  frequency, so this is also the frequency of the boot cycles Rhizo
  reports. Recording it needs no kernel change.

`bench.yml` publishes `runner` as a `bench-runner` annotation. The full
results in the `bench-results-N` pieces carry it too. On a failing gate,
the step that names the metrics also builds a runner line, the class and
identity from `bench-results.json` plus the gate's class line from the
log. The read-only report job puts it in the issue as `Runner: …`. It
receives the line through a job output and an environment variable,
never inside the script.

## Risks

- The model name does not tell two machines of one model in different
  host configurations (SMT, memory, noisy neighbours). The spread within
  Y over 11 runs (median 1.08x; worst noise 2.05x, Cloud Hypervisor's
  time to ready) is what the +100 % threshold already absorbs.
- Hosted pools change. A new model is an unknown class, gated against
  the default and reported as such until its baselines are added.
