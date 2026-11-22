# Design

## Evidence

The new property test (`many_domains_stay_within_one_stretched_slice`,
three to six domains) fails on `main` (default seed, 2,000 cases): three
domains on one CPU, the one of weight 4067 off its part by more than the
longest slice, 199.3 ms. A scratch copy of `Fair` on the host (not
committed) measured the worst distance from a domain's part, in longest
slices so far. Each row has 20,000 random cases of 1 to 300 slices, half
up to 20 nominal slices and half under 1/50 of one, on 1 to 4 queues:

| Domains | `main` | This change |
|---|---|---|
| 2 | 0.9999 | 0.9999 |
| 3 | 1.636 | 0.9999 |
| 4 | 2.179 | 0.9999 |
| 6 | 2.355 | 0.99995 |

A model of the pick in Python (scratch) found the same, and searched
harder for a counterexample to the fixed pick: hill climbing and beam
search over slice lengths (records after short slices, lengths up to
2,000 times apart) and weights from 1 to 10,000, three to six domains.
The worst it found was 0.99998 of the longest slice. That is a light
domain that ran one very long slice, `L × (1 − w / W)`, which the bound
allows.

## D1. Finish with the longest slice charged so far

### The cause

Write `S_j` for domain `j`'s virtual time (CPU time charged over weight
`w_j`) and `V` for the CPU's (the total over the sum `W` of the busy
domains' weights). The lag `λ_j = w_j (V − S_j)` is how far, in
nanoseconds, `j` is behind its weight's part; the lags sum to zero. A
pick takes, of the eligible domains (`S_j ≤ V`, so `λ_j ≥ 0`), the one
with the least finishing time `S_j + s / w_j`. Relative to `V` that is
`(s − λ_j) / w_j`. On `main`, `s` is the nominal slice, 10 ms, while the
charge counts the slice as long as it lasted:

- When slices last much longer than `s`, the term `s / w_j` no longer
  matters and the pick takes the least virtual time `S_j`. That is
  start-time fair queueing. It compares lags divided by weights, so a
  heavy domain far behind in nanoseconds waits behind light domains a
  little behind in virtual time. SFQ's bound between two domains is the
  sum of their slices, so a domain can be about two slices off.
- When slices are much shorter than `s`, the pick ranks eligible domains
  by weight, heaviest first. The light ones fall behind the same way.

WF2Q+ puts the length of the slice the domain is about to get into the
finishing time. A CPU slice's length is not known when it starts, but
the longest slice charged so far bounds every slice before it. With two
busy domains the pick never chooses: their lags sum to zero, so at most
one is eligible. That is why #156's test held.

### The fix

A domain's finishing time is `S_j + Λ / w_j`, with `Λ` the longest
slice charged so far on any CPU since the policy was put in (`Fair` is
made when cells start to share by weight). Each domain keeps
`Λ / w_j` (`Paced::slice`), so a pick computes nothing more. A charge
compares the slice with `Λ`. A longer one (`Fair::lengthen`, cold)
updates every domain's `slice`. That is O(domains) once per new record,
and records stop once slices stop growing.

### The bound

Busy domains, one CPU stepping through every queue (one clock), all
starting at the same virtual time. After slice `k`, write `L_k` for the
longest of the first `k` slices. Pick `t` uses `Λ_t = L_{t−1}`. Then, for
every domain `j`, `−L_k (1 − w_j / W) ≤ λ_j < L_k`: within one slice.

*Ahead.* `j` runs only when `λ_j ≥ 0`, and a slice `l` lowers its lag by
`l (1 − w_j / W)`. Between its runs its lag only grows.

*Behind.* Fix a domain `i` and the state after slice `k`. Let `S_i` be
its virtual time then. Let `s` be the last pick at or before `k` whose
domain `j'` had a finishing time above `S_i + Λ_s / w_i`. This is `i`'s
finishing time at `s`, with its current virtual time. Use `s = 0`, the
start, if no such pick exists.

1. `i` was not eligible at `s`. Its finishing time then was at most the
   one above, so below `j'`'s, and it would have been chosen. So
   `D = S_i − V_{s−1} > 0`.
2. `w_{j'} < w_i`, since `S_{j'} ≤ V_{s−1} < S_i` and `j'`'s finishing
   time is above `i`'s. So after `s`, `j'`'s finishing time stays above
   `i`'s as `Λ` grows, and `j'` does not run again before `k`.
3. Every domain `j ≠ i` that runs after `s` (last at `t ≤ k`) had a
   finishing time at most `i`'s there: `S_j(t) ≤ S_i + Λ_t (1/w_i − 1/w_j)`.
   Its virtual time just after `s` was above `V_{s−1}`. If
   `w_j ≤ w_i`, its finishing time at `s` was at most `i`'s (finishing
   times only grow), so below `j'`'s, and it was not eligible. If
   `w_j > w_i` and it was eligible, its finishing time was at least
   `j'`'s, so `S_j > S_i + Λ_s (1/w_i − 1/w_j) > S_i > V_{s−1}`. `i`
   itself was not eligible at `s` (1).
4. So, with `ρ_t = max(0, l_t − Λ_t)`, each such `j` ends at
   `S_j(k) ≤ S_i + Λ_t / w_i + ρ_t / w_j`. Its work after `s`,
   `w_j (S_j(k) − S_j(s))`, is below `w_j (D + Λ_t / w_i) + ρ_t`, and
   `i`'s own work after `s` is below `w_i D`.

The work from `s` on moves `V` by its total over `W`. Summing:

`W (V_k − S_i) = W (V_k − V_{s−1}) − W D < l_s + Σ ρ_t + (W − w_i) L_k / w_i − (W − Σ_run w_j) D`,

and the last term is negative (`j'` does not run again, so the domains
that run weigh less than `W`). Multiplying by `w_i / W`:

`λ_i < (w_i (l_s + Σ ρ_t) + (W − w_i) L_k) / W`.

A slice longer than all before it (`ρ_t > 0`) raises the longest slice
by `ρ_t`, so the `ρ_t` of distinct picks after `s` add up to at most
`L_k − L_s`, and `l_s ≤ L_s`. So `l_s + Σ ρ_t ≤ L_k`, and `λ_i < L_k`.
At `s = 0` the same steps give `λ_i ≤ L_k`.

The proof uses three properties of `Λ`. It never decreases (step 3). It
is at least every slice before the pick, so overruns add up to the
growth of the longest slice (step 4). It is at most the longest slice
yet, so `Λ_t ≤ L_k`. The nominal slice fails the second when slices are
stretched: that is the finding. A floor at the nominal slice
(`max(Λ, 10 ms)`) fails the third when slices are short: the model
reaches 1.25 slices then. A `Λ` that decays toward recent slices fails
the first.

Virtual times are fixed point. A domain's moves by `⌊ns · 2^16 / w⌋`
units per charge, the CPU's by the reciprocal of the sum, so `V` is the
weighted mean of the domains' times within rounding. The property tests
check the bound exactly, in nanoseconds, after every slice.

### Rejected

- **Changing D23's promise.** The true bound of `main`'s pick grows with
  the number of domains (1.9 longest slices at four in #156's
  exploration, more with short slices mixed in). A promise that depends
  on the number of domains says less than one that does not.
- **The domain's last slice.** It predicts the next slice no better than
  the nominal one. The model reaches 1.8 to 2.6 slices.
- **Picking the domain furthest behind in nanoseconds.** It reaches 1.4
  slices: a heavy domain behind a little falls further behind while a
  light one runs a long slice.

### Cost

The charge's comparison with `Λ` is two instructions (x86_64) or three
(AArch64) on the switch path while cells share by weight; round robin
does not run it. In the same change, the pick's fallback moves out of
the pick loop. When no domain is eligible, the pick takes the one with
the least virtual time and moves the CPU's clock to it. `main` tracked
that candidate for every domain at every pick. Now a second pass finds
it (`Fair::least`, cold) only when the first finds no eligible domain.
That cannot happen while every domain stays busy. The candidate is the
same, ties included. Measured with `--icount-only` against `main`
(0ab9b29): `cell.switch_domains_insns` 1,373 to 1,342 on x86_64 and 1,287
to 1,266 on AArch64. `cell.switch_insns` is unchanged, at 872 and 824.
Every other count is unchanged, except `workload.cold_insns` on AArch64
(−34 of 36.5 million) and `cell.resume_touch_insns` on x86_64 (−2 of 1.1
million).

## D2. Exact charge per domain in test builds

`domains_share_by_weight` compares a domain of 16 cells with a domain of
one. #156's `Fair::charged_ns` read a cell's virtual time times its
share. That is exact only for a cell alone in its domain: a cell's share
is rounded, and its arrivals move its virtual time. Summing it over 16
cells is neither exact nor what the policy balances. The domain level
is.

Each domain counts its charged nanoseconds (`Dom::charged`), and
`Fair::domain_charged_ns` reads it. The counter exists only with
`rhizo-sched`'s feature `ktest`, which the kernel's `ktest` turns on, and
in the crate's own tests. Benchmark and control builds do not count it,
so their switch path is unchanged. `Host::charged_ns(id)` now reads the
charge of `id`'s domain. For `root_weights_share_a_cpu`, whose roots are
alone in their domains, that is the same figure as before. `charged_ns`
by cell had no other user and goes. #156's property test asserts the
new reading instead, through a helper it shares with the new test.

Rejected: a counter in every build, which #156 rejected for its cost per
switch; and deriving the domain's charge from its virtual time, rounded
at every charge (`paced`), and moved by its arrivals.

`domains_share_by_weight` then steps for at least 2 s, as before, and
until the two domains' charged total is 20 times the longest slice
charged during the test. The longest slice is the most the total moves
in one step. With two busy domains each is within one slice `L` of half
the total `T` (D1). With `T ≥ 20 L`, the one-cell domain's share is
within 5 % of 50 %, so the truncated percentage is in `45..=55`, the
band of before. On an idle host, 20 slices take about 400 ms and the
test still lasts 2 s.

## Verification under load

A scratch build (not committed) ran only the two share tests, each eight
times in a row, in `ktest-smp` with busy loops on a 4-CPU host. It
printed each run's shares and longest slice.

| Arch | Busy loops | One-cell domain (`domains_share_by_weight`) | Weight 300 (`root_weights_share_a_cpu`) | Longest slice |
|---|---|---|---|---|
| x86_64 | 10 | 49 to 50 % | 73 to 75 % | 23 to 52 ms |
| x86_64 | 16 | 49 to 51 % | 73 to 75 % | 41 to 111 ms |
| AArch64 | 12 | 49 to 50 % | 72 to 75 % | 36 to 92 ms |

Every run passed.
