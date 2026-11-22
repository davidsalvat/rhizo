## ADDED Requirements

### Requirement: Runner identity in the results
Every benchmark result SHALL record the machine that measured it: the
host CPU's model name, family, model, stepping and microcode, the number
of CPUs, whether KVM is available, whether a guest saw performance
counters, and the TSC frequency the host's kernel found at boot (unknown
where its log cannot be read). The `bench` workflow SHALL publish it as an
annotation.

#### Scenario: Results name their runner
- **WHEN** `cargo xtask bench --output bench-results.json` runs
- **THEN** `bench-results.json` holds a `runner` object with the CPU's model name, family, model, stepping, microcode, the CPU count, KVM, guest PMU and TSC frequency

#### Scenario: Results stored before runners were recorded
- **WHEN** stored results without a `runner` are given to `cargo xtask bench --from`
- **THEN** they load, and their class is unknown

### Requirement: Runner class
A runner's class SHALL be its CPU model name in lower case, with `(R)` and
`(TM)` dropped and every run of characters other than ASCII letters and
digits replaced by one `-`, and the results SHALL carry it with the
runner.

#### Scenario: Class of a model name
- **WHEN** the benchmarks run on a host whose CPU is `Intel(R) Xeon(R) Platinum 8370C CPU @ 2.80GHz`
- **THEN** the results' runner class is `intel-xeon-platinum-8370c-cpu-2-80ghz`

### Requirement: Timing gate per runner class
The baseline SHALL be able to hold, per environment, timing baselines of
a runner class (`classes.<env>.<class>.<metric>`). The gate SHALL compare
a run's timings with the values of its own class where the class has
them, and with the default values of the environment otherwise.
Deterministic metrics and instruction counts under `-icount` SHALL always
be compared with the default.

#### Scenario: A slow class within its own baselines
- **WHEN** a run on class `z` measures `cell.fork_with_files_us/rhizo/qemu-microvm` at 0.9 µs, the default baseline is 0.4 µs and class `z`'s is 0.66 µs
- **THEN** that metric passes the `ci` gate (+36 % on its class)

#### Scenario: A regression on a slow class
- **WHEN** the same run measures 1.4 µs
- **THEN** the gate fails naming the metric and runner class `z`

#### Scenario: Instruction counts ignore the class
- **WHEN** a run on class `z` counts more instructions under `-icount` than the default threshold allows
- **THEN** the gate fails whatever class `z` holds

### Requirement: Unknown runner class
A run whose class has no baselines SHALL be gated against the default
baseline of the environment, and the gate SHALL say so.

#### Scenario: Unknown class
- **WHEN** a run's CPU model has no baselines of its own, or its results do not name the runner
- **THEN** its timings are gated against the default baseline and the gate prints `unknown runner class: <model>; compared against the default class` (`not recorded` for the model when the results do not name the runner)

### Requirement: Runner-class baselines hold timings only
A class SHALL hold only timings that the default baseline of the
environment also has, and the gate SHALL fail on any other entry of a
class.

#### Scenario: A class may not loosen other gates
- **WHEN** a class holds an `-icount` count, a deterministic metric, or a metric that the default baseline lacks
- **THEN** `cargo xtask bench --check` fails naming each such entry

### Requirement: New runner classes are explicit
Runner-class baselines SHALL change only through a reviewed baseline
change. The gate SHALL never add or raise them by itself.
`cargo xtask bench --update-baseline --class <c>` SHALL only create the
baselines of a class that has none. It SHALL take them from results
measured on class `<c>`, and only the timings the default baseline has.
It SHALL refuse results from another class and SHALL refuse a class that
already has baselines.

#### Scenario: Creating a class
- **WHEN** `cargo xtask bench --from results.json --env ci --update-baseline --class c` runs with results measured on class `c`, which has no baselines
- **THEN** `classes.ci.c` holds those results' timings that `environments.ci` has, and nothing else changes

#### Scenario: Changing an existing class
- **WHEN** the same command runs for a class that already has baselines, or with results measured on another class
- **THEN** it fails and the baseline file is unchanged

## MODIFIED Requirements

### Requirement: Failing timing gate reported
When the benchmark gate fails on main or in the scheduled run, CI SHALL
open an issue, or comment on the open one, naming each failing metric, the
run, the runner (its class, CPU model and what its timings were compared
against), and the commits since the last successful benchmark run on main.
Only that reporting step SHALL have write access to issues.

#### Scenario: Regression on main
- **WHEN** the gate on main fails on `cell.fork_us/rhizo/firecracker`
- **THEN** the timing-gate issue names that metric, the runner's class and CPU model, links the run, and lists the commits since the last green benchmark run

#### Scenario: Runner of an unknown class
- **WHEN** the gate fails on a runner whose class has no baselines
- **THEN** the issue says `unknown runner class: <model>; compared against the default class`
