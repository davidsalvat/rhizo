# Proposal

## Why

`a_cpython_agent_talks_forks_and_survives_suspend` failed once in CI
(GitHub Actions run 44941595429, job 136959808283, "AArch64 tests, part 2
of 2", QEMU `virt` under TCG, on PR #128, which changed only library code
the kernel does not use) with `suspended after every turn`: 2 suspends for
3 turns. The rerun passed.

The cause is in the host side of the agent workload (`kernel/src/cell/agent.rs`),
not in the agent or the network: the host recognized a turn's end only
when the *last* line of the cell's output was that turn's line, and it
read the output only between the cell's slices. After its last turn the
agent prints the turn's line, sleeps 1 ms and prints `fork` with no
network call in between. When at least 1 ms of guest time passes between
CPython reading the clock for that sleep and the kernel blocking it (a
descheduled TCG vCPU on a loaded runner: the failing job ran in-kernel
tests next to the host tests and the Node.js step), the sleep is already
due, the thread goes on in the same slice, and the host first sees
`turn 3 …\nfork\n`. It took the decision point and skipped the suspend.
The full analysis and the reproduction are in `design.md`.

## What Changes

- **Every turn line is read.** The host reads the agent's output with a
  line reader (`rhizo_core::agent::Watch`) that finds every complete line
  starting `turn ` and the line ending in `fork`, however the bytes are
  split, and suspends after each turn even when the decision point came in
  the same piece. It freezes the agent once it has seen the decision point
  and the agent sleeps, as before.
- **Two turns unseen is an error that says so**, instead of a short count:
  structurally it cannot happen (see design D2), and if it ever did the
  run reports which turns ended before the host looked.
- **Tests**: host tests of the reader, a property test with arbitrary
  splits of the agent's output, and a unit test of the exact piece CI saw.

### Out of scope

Changing the agent (`cells/python/agent_loop.py`) to wait for the host
between turns; the in-kernel test and its assertions are unchanged.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workloads`: the agent is suspended after every turn however its output
  reaches the host.

## Impact

`crates/rhizo-core/src/agent.rs` (new, host-tested), `kernel/src/cell/agent.rs`.
The benchmarked agent figures measure the same suspends and forks; one
byte loop over the output the host already copies.
