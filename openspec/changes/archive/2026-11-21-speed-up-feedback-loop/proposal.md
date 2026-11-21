# Speed up the feedback loop

## Why

Every change in this repository is driven by agents that run
`cargo xtask test` before they push, often on both architectures, and wait
for CI afterwards. Measured on a 4-CPU host under emulation, a full
x86_64 `test` took about 6 minutes, nearly all of it VMs running one after
another (host tests 45 s, `ktest` 92 s, `ktest-smp` 108 s, self-checks
15 s) while most CPUs sat idle. That wait is the slowest part of each
iteration.

The benchmark gate also passed on `main` while measuring no cell figure at
all: a crash of the cell benchmark VM is reported as a skipped step, and a
missing figure was not compared with anything. The crash itself is the
50 MiB fork benchmark running out of memory in a 128 MiB VM once the kernel
image grew (dynamic-linking test files, relinked CPython).

## What Changes

- `cargo xtask test` builds every kernel it needs first, then runs the test
  steps at once on the host's CPUs (host tests, boot check, `ktest`,
  `ktest-smp`, self-checks), each step taking as many host CPUs as it has
  virtual CPUs, at most half the host, so no step runs much slower than
  alone. Output of a step is shown only when it fails. `RHIZO_JOBS=N` sets
  the CPUs used; VMMs that share a host device (the tap of Firecracker and
  Cloud Hypervisor) and `--gdb` keep running one step at a time.
- A new boot option `shard=I/N` makes an in-kernel test build run only the
  tests whose position is `I` modulo `N`; `RHIZO_SHARDS=N` splits `ktest`
  and `ktest-smp` over that many VMs. It is opt-in, because splitting
  exposed tests that depend on the tests before them; three were fixed
  here, the rest are listed in design.md.
- The hang limit (time without console output) doubles while steps share
  the host.
- Tests made independent of their order and of host load:
  - the network card comes up at boot (`cell::init`) instead of with the
    first host, so its frames are never counted against a test;
  - `cells_in_parallel_stay_isolated` runs once before it measures, as
    `assert_no_leak` does;
  - `sleeping_cell_uses_no_cpu_and_wakes_on_time` sleeps 500 ms instead of
    20 ms, so the other cell finishes first on a loaded host. Same
    assertions.
- The benchmark gate fails when a gated figure in the baseline was not
  measured on a VMM that ran (a crash or a skipped step), naming the skip
  reason.
- The cell benchmark VM gets 256 MiB, so the 50 MiB fork benchmark fits
  again.

## Impact

- Specs: `developer-feedback` (parallel test steps, splitting), `benchmarks`
  (missing figures fail the gate), `kernel-testing` (`shard=`).
- Code: `xtask/src/{main,vmm,bench,workloads}.rs`,
  `crates/rhizo-core/src/{options,ktest}.rs`, `kernel/src/{main,cell,tests}.rs`.
- Measured: full x86_64 `cargo xtask test` 6 min to about 2.5 min on a
  4-CPU host shared with other builds.
