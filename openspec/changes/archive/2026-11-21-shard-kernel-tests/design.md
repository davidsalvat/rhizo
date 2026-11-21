# Design

## D1. The root cause: a connection still closing

`dynamically_linked_programs_behave_as_on_linux` measures
`memory_balance()` (free frames and frames the heap holds, and the heap's
live allocations, with the kept images dropped) before and after itself.
Instrumented in the failing split (`RHIZO_SHARDS=2 cargo xtask --arch
aarch64 test --only ktest`, part 2 of 2), the two measures were:

| Measure | Balance | `wire_busy()` | Open sockets | Kept pages |
|---|---|---|---|---|
| before | (119062528, 1040) | true | 0 | 0 |
| after | (119062528, 1038) | false | 0 | 0 |

The test runs right after
`a_silent_connection_outlasts_the_timeout_and_reconnects_at_once` (in this
split, with only `cpython_in_a_cell`, not run without CPython, between
them). That test's cell closes its last connection to the host's echo
server through the network card and exits; `smoltcp` keeps the closed
connection until the peer acknowledges the close, and with it two heap
allocations. It finished closing while the dynamic programs ran, so the
count after was 2 lower. In the full order,
`stack_grows_on_touch_and_returns_its_frames` runs cells in between (its
check counts frames, not heap allocations), and the close finishes during
it; in other splits the timing differs again.

The kept images and file pages, the other shared caches, were already
settled: `memory_balance()` drops them first.

## D2. The fix: settle the card before measuring

`memory_balance()` is the one helper every leak check uses (dynamic
linking, Node.js, packages, density, isolation, quotas, leaks, images). It
now calls `settle_wire()` first: it polls the card through a `Host` of
its own until no connection through it is open or closing
(`crate::cell::wire_busy()`), for at most 10 seconds, and fails the test
otherwise. This is the loop the CPython agent and package tests already
ran at their end; they now call the helper.

The check is not loosened: both measures are taken with nothing closing,
so the count compares the test's own allocations exactly. A connection a
test leaves open or never finishes closing fails its second measure
("the card's connections closed"); an allocation it leaks fails "nothing
leaked" as before. Without a card (`wire_busy()` false) the helper costs
one `Host::new()`.

Rejected: making the net test wait for its own close. It fixes this pair
only; the next test that leaves a connection closing would break a leak
check in some split again, while the helper protects every leak check
from every earlier test.

## D3. Shares dealt by cost

`shard=I/N` ran the tests whose position was `I` modulo `N`. The first CI
run with shards (44937949408) showed how uneven that is: the two `ktest`
VMs took 30.4 s and 75.6 s, the two `ktest-smp` VMs 25.0 s and 64.6 s, so
the parts barely shortened (VM phases 80.7 s and 95.7 s against 81.7 s
and 98.2 s). A few expensive tests decide it: on AArch64 (one run each,
live output) the suite takes 118 s on one CPU and 100 s on four, and its
most expensive test 11.9 s and 18.0 s.

`TestCase` now has a `cost` (estimated tenths of a second; 1 unless given,
`ktest_case!(name, 120)`). The 19 tests that take 3 s or more on AArch64,
one-CPU time plus four-CPU time, carry that sum. `run` ranks the cases by
cost, most expensive first, ties by position, and deals the ranks to the
shares 0 to N-1, then N-1 to 0, and again; each share runs its cases in
suite order. A case's rank is a count over the suites, so the deal needs
no allocation (the runner is `no_std` without `alloc`); it costs
`O(n^2)` comparisons, about 43000 for 147 tests (one pass to count, one
to run). It depends only on the suites and `N`, so the shares of one `N`
still hold every test once.

Simulated with the measured durations, two shares get 60.9 and 57.3 s on
one CPU (modulo: 70.2 and 48.0) and 55.9 and 44.5 s on four (63.0 and
37.3); a cost table with every test (a longest-processing-time deal) would
get 59.1 each, at the price of a cost on all 147. Measured on AArch64
after the change: `ktest` 38.8 and 42.1 s (was 27.6 and 72.5),
`ktest-smp` 46.2 and 46.8 s (was 32.6 and 75.1).

A new test without a cost counts 1: it only makes the split less even,
never wrong. The costs are AArch64's, where CI splits; x86_64 splits less
evenly (77.1 and 40.9 s for `ktest` locally).

## D4. Every split

After D2 and D3, every VM passes for `ktest` and `ktest-smp` with
`RHIZO_SHARDS` 1 to 4, on both architectures (development host, 4 CPUs,
no KVM, shared with other builds, so the times are only indicative;
`ktest-smp` takes at most `parallel / 2` VMs, so its 3 and 4 ran with
`RHIZO_JOBS=8`). Seconds per VM:

| Step | 1 | 2 | 3 | 4 |
|---|---|---|---|---|
| AArch64 `ktest` | 75.4 | 38.8, 42.1 | 42.1, 24.1, 44.6 | 31.5, 19.7, 23.5, 8.4 |
| AArch64 `ktest-smp` | 81.8 | 46.2, 46.8 | 50.0, 27.6, 35.7 | 35.9, 25.7, 35.3, 19.8 |
| x86_64 `ktest` | 107.4 | 40.9, 77.1 | 78.5, 32.5, 43.5 | 53.5, 57.6, 56.1, 16.2 |
| x86_64 `ktest-smp` | 130.9 | 62.4, 92.0 | 56.5, 17.3, 28.8 | 25.7, 20.1, 38.3, 8.6 |

The same matrix with the modulo deal and D2 (before D3) also passed
everywhere. No other order dependence showed.

## D5. CI: two VMs per in-kernel step on AArch64

`ci-arm64` sets `RHIZO_SHARDS=2`. On its 4-CPU runner that splits `ktest`
(part 2) into two one-CPU VMs and `ktest-smp` (part 1) into two four-CPU
VMs, each counted as 2 host CPUs by the scheduler (`Job::weight`), beside
the other steps. The parts' deal (`xtask/src/parts.rs`) is unchanged: it
deals steps, not VMs, so every step still runs in exactly one part and
every test in exactly one VM of it. The x86_64 jobs keep one VM per step.

A failing VM of a split step prints `RHIZO_SHARDS=N` in its `reproduce`
command, so the failure reproduces with the same split.

## Result

Code pull-request runs, from `gh api
repos/davidsalvat/rhizo/actions/runs/<id>/jobs` (minutes) and the job
logs (VM phase: from the last build to the last VM step). Baseline: the
pull request of `split-hotspot-files` (44935986869) and main after it
(44936411036). After: this pull request with D2, D3 and D5 (44940305817,
two attempts).

| Job | 44935986869 | 44936411036 | 44940305817 #1 | 44940305817 #2 |
|---|---|---|---|---|
| AArch64 part 1 | 2.87 (VM 81.7 s) | 2.77 (VM 81.8 s) | 2.65 (VM 77.1 s) | 2.85 (VM 78.7 s) |
| AArch64 part 2 | 2.97 (VM 98.2 s) | 3.07 (VM 95.8 s) | 3.00 (VM 91.2 s) | 2.90 (VM 90.4 s) |
| `icount-aarch64` | 3.35 | 3.43 | 3.82 | 3.33 |
| `ci` (x86_64 tests) | 3.40 | 3.22 | 3.12 | 3.03 |
| **Wall clock** | **3.60** | **3.77** | **4.13** | **3.63** |

The in-kernel tests left the end of both parts: part 2's VM phase is 6 to
8 s shorter and ends with the host tests and Node.js, part 1's 3 to 5 s
shorter and ends with the control plane. The gain is small because the VM
phase of each part is CPU-bound, not bound by one VM: part 2 keeps its 4
CPUs busy with about 300 CPU-seconds (host tests 2 CPUs for 71 s, the
in-kernel tests about 100, Node.js 2 CPUs for 27 s), so about 75 s is
its floor, and Node.js now waits for the two in-kernel VMs. The run's
wall clock is set by the AArch64 instruction counts and the x86_64 tests
(3.0 to 3.8 minutes), as after `faster-ci-2`, and is unchanged within the
runners' noise.

What would shorten the AArch64 tests further, not done here: a third
AArch64 runner (`--part K/3`, about 190 CPU-seconds and a 50 s VM phase
each, one more ARM64 runner per code pull request), which only pays once
the instruction counts and the x86_64 tests are shorter too.
