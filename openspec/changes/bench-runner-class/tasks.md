# Tasks

## 1. Evidence

- [x] 1.1 Tabulate boot cycles, `cell.syscall_ns`, guest PMU and the noise metrics over every completed `bench` run on main (`b696efa` to `e655beb`, job logs); verify the three classes of issue #121 and that the logs name no CPU model (design, evidence)

## 2. Runner identity

- [x] 2.1 `Runner` from `/proc/cpuinfo` (model name, family, model, stepping, microcode, CPUs; AArch64's implementer and part), KVM, guest PMU from the results, TSC frequency from the host kernel's log, in `bench-results.json` and the printed table (D5); verify `cargo test -p xtask bench` (`runner_identity_comes_from_cpuinfo`, `tsc_frequency_comes_from_the_kernel_log`, `guest_pmu_is_read_from_the_results`, `results_round_trip_through_json`)
- [x] 2.2 The class key, the normalized model name (D1); verify `class_key_normalizes_the_model_name`

## 3. Gate per class

- [x] 3.1 `classes.<env>.<class>` re-values timings of the default only; deterministic and `-icount` figures always from the default (D2); verify `a_runner_class_gates_timings_against_its_own_baselines`, `a_class_only_re_values_default_timings`
- [x] 3.2 Unknown classes gate against the default and say so (D3); verify `an_unknown_runner_class_is_gated_against_the_default_and_says_so` and the existing gate tests unchanged
- [x] 3.3 `--update-baseline --class` creates a new class from a run on it, never changes one (D4); verify `a_new_class_is_created_once_from_a_run_on_it`, `options_parse`

## 4. CI and documentation

- [x] 4.1 `bench.yml`: `bench-runner` annotation in a step of its own, a publishing step for pieces 40 to 49 (main's results already took 40, and `runner` comes after `metrics`); the failing gate's runner line in the timing-gate issue (D5); verify `actionlint`
- [x] 4.2 `bench/README.md`: runner identity, classes, adding a class needs the owner's approval; verify `openspec validate bench-runner-class --strict`
- [x] 4.3 Verify `cargo xtask check`
- [x] 4.4 Dispatch `bench.yml` on the branch; verify the run records its runner (annotation `bench-runner`) and the gate prints its class line (run 44962453021: `intel-xeon-6975p-c`, class Y by its figures, unknown to the baseline, gate green)
- [ ] 4.5 After merge, the owner adds the class baselines proposed in the pull request (a reviewed baseline change, not part of this change)
