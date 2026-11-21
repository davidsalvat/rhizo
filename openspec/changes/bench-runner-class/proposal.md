# Proposal

## Why

The timing gate of `bench.yml` (environment `ci`, +100 %) fails on runner
noise. GitHub's `ubuntu-24.04` runners fall into hardware classes of very
different speed, and the `ci` baselines were set on one of them. The
analysis in issue #121 (comment 7070033038), over every `bench` run on main
since `b696efa`, finds three classes. They are told apart by the boot TSC
cycles (`boot.kernel_cycles` on QEMU `microvm`), by `cell.syscall_ns` and by
whether the guest sees a PMU:

| Class | Boot cycles (microvm) | `cell.syscall_ns` | Guest PMU | Runs on main |
|---|---|---|---|---|
| X | 4.35 to 4.36 M | 52 to 53 | no | 3 |
| Y (the baselines) | 6.35 to 6.77 M | 68 | yes | 11 |
| Z | 5.11 to 5.64 M | 62 to 64 | no | 3 |

On class Z, `cell.fork_with_files_us` (every VMM), the agent turns,
`laptop.resume_turn_us` and the control path on Cloud Hypervisor read 2.0
to 2.4 times their baseline, on every VMM at once. That is the runner, not
the code: the runs on `693be50`, `89c1cee` and `2a3beec` failed on them,
and the same code passed on class Y. Every such failure costs an
investigation and a rerun. A wider threshold would hide real regressions on
the fast classes.

Nothing in the results says which machine measured them. The bench logs
hold no CPU model, so the classes above had to be inferred from the
figures.

## What Changes

- Every bench result records its runner's identity in
  `bench-results.json` (`runner`): the CPU model name, family, model,
  stepping and microcode from `/proc/cpuinfo`, the CPU count, KVM, whether
  a guest saw a PMU, and the TSC frequency the host's kernel found at boot.
  `bench.yml` publishes it as a `bench-runner` annotation, next to the
  `bench-results-N` pieces, which carry it too.
- The runner's class is its CPU model name, normalized
  (`intel-xeon-platinum-8370c-cpu-2-80ghz`). No heuristic on the figures.
- `bench/baseline.json` can hold timing baselines per class,
  `classes.<env>.<class>.<metric>`. A run whose class has baselines is
  compared with them for its timings. Deterministic metrics and `-icount`
  instruction counts are always compared with the default. A class may
  re-value only timings that the default baseline has, and the gate fails
  on any other entry.
- A run whose class has no baselines is gated against the default, as
  today, and the gate says so: "unknown runner class: <model>; compared
  against the default class".
- `cargo xtask bench --env ci --update-baseline --class <c>` creates the
  baselines of a new class from a run measured on that class. It refuses
  results from another class and refuses to change a class that exists.
  Like any baseline change, it needs the owner's approval in its pull
  request.
- The timing-gate issue report names the runner, its class and what it
  was compared against.

### Out of scope

- No baseline values change: `bench/baseline.json` is untouched, and
  everything works with the default class alone. The values proposed for
  classes X, Y and Z, with their evidence, go to the project owner in the
  pull request. The class keys of X, Y and Z are not known yet: past runs
  did not record their model.
- The thresholds stay as they are.
- `dedicated` and `ci-arm` work as before: one machine, and instruction
  counts only, respectively.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `benchmarks`: results name their runner; timings are gated per runner
  class; the timing-gate report names the class.

## Impact

`xtask/src/bench.rs` (results, gate, `--class`), `xtask/src/lib.rs`
(help), `.github/workflows/bench.yml` (annotation, issue report),
`bench/README.md`. No kernel change and no new dependency. Reading the
TSC frequency runs `dmesg`, or `sudo -n dmesg` where the log is
restricted (as on GitHub's runners). Without either, the frequency is
recorded as unknown.
