# Proposal

## Why

Design D23 of `add-cell-hierarchy` says each busy domain keeps within
one slice of its weight's part of the CPU. Change `fix-share-test` (#156)
showed that a busy host stretches slices under TCG (a 20 ms slice can
last 50 to 190 ms), and that each slice is charged as long as it lasted.
It checked two domains with slices of any length, and they held. It also
found that with three or more domains and slices longer than the nominal
10 ms, a domain can drift up to about 1.9 of the longest slices from its
part. That breaks D23's promise and the spec requirement "Shares by
weight hold as charged", which says one slice whatever the slices'
lengths.

The cause is in the domain pick (design, D1). A domain's finishing
virtual time counts a nominal slice of 10 ms. The charge counts the
slice as long as it lasted. WF2Q+ needs the finishing time to count the
slice the domain is about to get. When slices are much longer than
10 ms, the pick ranks eligible domains by virtual time alone. That
ignores how far a heavy domain is behind in nanoseconds. When slices are
much shorter, it ranks them by weight alone. Either way, a domain can
stray about two slices. With two domains the pick never chooses: at most
one of two busy domains is behind, and eligibility alone picks it. That
is why #156's two-domain test held.

The in-kernel test `domains_share_by_weight` measures the way #156 fixed
for `root_weights_share_a_cpu`. It runs 2 s of wall time against a 5 %
band, so on a busy host it fails for the same reason. Its domain of 16
cells needs a charge per domain, not per cell.

## What Changes

- A domain's finishing virtual time counts the longest slice charged so
  far (`Fair::longest`), not the nominal slice. With that, each busy
  domain stays within one slice (the longest yet) of its weight's part,
  whatever the number of domains and the slices' lengths (proof in the
  design, D1). A longer slice than any before updates each domain's
  slice once, off the switch path. The check costs a comparison per
  charge. Moving the pick's fallback, used only when no domain is
  eligible, out of the pick loop pays for it.
- A host property test, `many_domains_stay_within_one_stretched_slice`:
  three to six busy domains of any weights, slices of any length (up to
  20 nominal slices, mixed with short ones), one CPU stepping through 1
  to 4 queues. After every slice, each domain is within the longest slice
  so far of its weight's part.
- Each domain's charged CPU time, counted exactly in in-kernel test
  builds only (`rhizo-sched` feature `ktest`, which the kernel's `ktest`
  turns on): `Fair::domain_charged_ns`, and `Host::charged_ns` now reads
  the charge of the cell's domain. Other builds do not count it, so a
  switch costs nothing more. It replaces #156's `Fair::charged_ns`,
  which read a cell's virtual time and was exact only for a cell alone
  in its domain.
- `domains_share_by_weight` compares the two domains' charged time over
  at least 2 s and at least 20 longest slices, as #156 did. The 45 to
  55 % band is unchanged.

### Out of scope

- Several CPUs that pick each by their own clock: D23 says fairness holds
  per CPU and over balancing periods. The bound is proved and tested for
  one CPU stepping through every queue (`Host::step`, the tests).
- Not charging time stolen by the host (#156: the guest has no steal
  clock).

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `scheduling`: shares by weight hold within one slice for any number of
  domains, and the domain test is measured as charged.

## Impact

`crates/rhizo-sched/src/fair.rs` (the domain pick, the exact charge per
domain in test builds, a property test) and its `Cargo.toml` (feature
`ktest`); `kernel/Cargo.toml` (`ktest` turns it on);
`kernel/src/cell/shares.rs` (`Host::charged_ns`) and
`kernel/src/tests/shares.rs` (one test). No format or protocol change.
