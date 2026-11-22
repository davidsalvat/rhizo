# Proposal

## Why

The in-kernel test `root_weights_share_a_cpu` (#138, design D23 of
`add-cell-hierarchy`) failed once on x86_64 with 4 CPUs, with "weight 300
got 69 %", while other test steps kept the host busy. It passed when it
ran alone. A test that fails depending on the host's load is a wrong
test, or a scheduler that is wrong under that load. Which one it is
decides the fix.

The test steps two spinning roots of weights 300 and 100 on one CPU for
1 s of wall time, then checks that the heavier root's share of their
CPU-quota time is between 70 and 80 %. Reproduced under host load
(design, evidence), the scheduler keeps its promise: each domain's
charged time stays within one slice of its weight's part. The slices
are what change. When QEMU's vCPU thread is not running, the guest's
clock still advances, and the timer tick that ends a slice arrives late,
so a slice of up to 20 ms lasts 50 to 190 ms. The 1 s window then holds
6 to 13 slices instead of about 50. One slice is then 8 to 15 % of the
total, so "within one slice" no longer fits within 5 %. Under the same
load the scheduler's own charges and the cells' CPU-quota meters agree,
and both give 66 % in the worst run. The measurement is what fails, not
the policy.

## What Changes

- `root_weights_share_a_cpu` measures what the policy controls: the CPU
  time the scheduler charged each root (`Host::charged_ns`, in-kernel
  tests only, read from `rhizo_sched::fair::Fair::charged_ns`). It runs
  for at least 1 s and until the charged total is at least 20 times the
  longest slice charged. D23 keeps each of two domains within one slice
  of its weight's part, so 20 slices hold the share within 5 %, and the
  70 to 80 % bound is unchanged. On an idle host the test still takes
  1 s. On a busy one it runs longer, as long as its slices are.
- A host property test in `rhizo-sched`
  (`stretched_slices_stay_within_one_slice`): two busy domains, any
  weights, slices of any length up to 20 nominal slices, one CPU stepping
  through 1 to 4 queues. After every slice, each domain is within the
  longest slice so far of its weight's part, and `charged_ns` reads
  exactly what was charged.

### Out of scope

- Not charging time stolen by the host. Without a steal clock the guest
  cannot tell it apart, and the scheduler charges each slice as long as
  it lasted, as D23 says.
- `domains_share_by_weight` measures the same way over 2 s (100 slices
  when idle), and a busy host stretches its slices the same way. Its
  domain of 16 cells reads `charged_ns` only approximately, so it is left
  for a follow-up.
- With three or more domains and slices longer than the nominal one, the
  property above can exceed one slice (up to about two longest slices in
  a host exploration). D23's properties use nominal slices only. No
  in-kernel test depends on this; it is reported for a follow-up.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `scheduling`: shares by weight are stated and checked as charged,
  within one slice of any length.

## Impact

`kernel/src/tests/shares.rs` (one test), a test-only accessor in
`kernel/src/cell/shares.rs`, and in `crates/rhizo-sched/src/fair.rs` an
accessor and a property test. Nothing on the switch path changes, and
there is no format or protocol change.
