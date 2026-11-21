# Design

## D1. A run's time limit bounds the boot CPU's wait

`Host::run_until(at, stop)` stores `at` for the length of the run and runs
`run(|h| now_ns() >= at || stop(h))`. When the boot CPU has nothing to run
(`wait_or_poll`), it waits until the earlier of its own wake time and `at`,
or until `at` when it would otherwise wait for a wake-up interrupt only.
At `at` it wakes, finds its stop condition true, and ends the run as
before (the other CPUs give their cells back within a slice).

The `IdleTimer` bookkeeping does not change: the limit is the host's, not a
cell's deadline, so it is not handed between idle CPUs and no other CPU
wakes for it. A run whose cells can never run again still ends at once
(`Take::Done` with no cell running), not at its limit.

Alternatives:

- Let the boot CPU always program its timer for the earliest cell
  deadline, as with one CPU. The run would still outlive its limit until
  that deadline (an hour here, cut to the hardware's longest wait: 34 s on
  AArch64 at 62.5 MHz, 68 s on x86_64), and every deadline would wake two
  CPUs.
- Make the boot CPU poll its stop condition every slice (`look_every`, the
  control kernel's): wake-ups for nothing, at every slice, on an idle
  machine, which `add-tickless-idle` removed.
- Hand the stop condition a time through `run`'s signature: every caller
  of `run` changes, while three use a time.

## D2. Templates are frozen asleep

The server workload waits for `ready` as before, then steps the template
until `Host::asleep` holds, then freezes it. `asleep` is the host's safe
point to suspend or freeze a cell (spec `workloads`, design D3 of
`add-agent-workload`); the laptop workload already waits for it. A worker
prints `ready` and sleeps at once, so this costs a step or two at setup,
outside every measurement.

Alternative: expire, in `spawn_from_template`, the first sleep a fork
starts. A template does not say which sleep is its warm-up, and the spec
`fork` already defines templates as keeping no time from the moment they
are frozen.

## D3. Tests

`a_run_with_a_time_limit_ends_on_time` (in `ktest` and `ktest-smp`): one
`sleeper` per CPU sleeps an hour; 20 runs limited to 50 ms each must each
end within 500 ms (room for an emulated machine on a busy host; the
hardware's longest wait is 2 s or more), and the sleepers still sleep
afterwards.
On 4 vCPUs the first CPU to idle holds the hour; whenever it is not the
boot CPU, a run without D1 never ends (`ktest-smp` timed out at this test
before the fix), and when it is, the run lasts until the hardware's
longest wait. It is deterministic: no window has to be hit.

The template fix is shown by the stability runs (tasks): the 4-vCPU server
boot of `cargo xtask --arch x86_64 bench --workload server --cpus 4`
looped 35 times under QEMU TCG without a hang, every load ending by its
8 s limit and no worker left in the hour's sleep, where before about 1 boot
in 8 hung or left the 4,000 `serve` and `ping` workers asleep. (Under TCG
a busy host can still leave a few hundred workers unfinished at the
limit: 4 of the 35 boots, run while the host also ran the test suite.)

## D4. `--cpus` for the server workload

`--cpus N` (1 to 4) sets the server VM's virtual CPUs. It changes the
figures, so the baselines keep 1 vCPU: `--check` and `--update-baseline`
refuse it.
