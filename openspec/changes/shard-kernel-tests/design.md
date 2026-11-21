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

## D3. Every split

After the fix, every in-kernel test VM passes for `ktest` and `ktest-smp`
with `RHIZO_SHARDS` 1 to 4, on both architectures (development host, 4
CPUs, no KVM, shared with other builds; `ktest-smp` takes at most
`parallel / 2` VMs, so 3 and 4 ran with `RHIZO_JOBS=8`):

RESULTS_TABLE

No other order dependence showed.

## D4. CI: two VMs per AArch64 part

`ci-arm64` sets `RHIZO_SHARDS=2`. On its 4-CPU runner that splits `ktest`
(part 2) into two one-CPU VMs and `ktest-smp` (part 1) into two four-CPU
VMs, each counted as 2 host CPUs by the scheduler (`Job::weight`), the
other steps beside them. The parts' deal (`xtask/src/parts.rs`) is
unchanged: it deals steps, not VMs, so every step still runs in exactly
one part and every test in exactly one VM of it. The x86_64 jobs keep one
VM per step.

A failing VM of a split step now prints `RHIZO_SHARDS=N` in its
`reproduce` command, so the failure reproduces with the same split.

The modulo deal (`rhizo_core::ktest::run`) is uneven: on AArch64 the two
`ktest` VMs took 27.6 s and 72.5 s locally. A deal by test cost would
even them out; it is left out here (the uneven split already removes the
in-kernel tests from the tail of part 2).

## Result

RESULT_TABLE
