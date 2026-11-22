# Design

## Evidence

The reproduction used a scratch copy of the test, not committed. It ran
the test's setup 12 to 30 times in a row in `ktest-smp` (x86_64, 4
vCPUs, TCG, a 4-CPU host). After each step it recorded every root's
CPU-quota time (`cpu_ns`) and, through a counter added in `give_back`,
the time the scheduler charged. Load came from busy loops on the host.

| Host | Runs | Heavy root's share (`cpu_ns`) | Slices in 1 s | Longest slice | Outside 70–80 % | Error above the longest slice |
|---|---|---|---|---|---|---|
| idle | 12 | 73.9 to 74.8 % | 50 to 51 | 19 to 21 ms | 0 | 0 |
| 6 busy loops | 12 | 65.6 to 77.0 % | 28 to 51 | up to 139 ms | 1 (65.6 %) | 0 |
| 6 busy loops, with the scheduler's charge | 12 | 72.8 to 75.6 % (charge: 72.8 to 74.9 %) | 33 to 51 | up to 76 ms | 0 | 0 |
| 10 busy loops, with the scheduler's charge | 30 | 66.4 to 82.0 % (charge: 66.4 to 77.4 %) | 6 to 51 | up to 187 ms | 2 (66.4 %, 82.0 %) | 0 |

The error is |a − ¾(a + b)|, the heavy root's distance from its weight's
part. The 66.4 % run had 13 slices: the longest was 127 ms and the error
85 ms. The scheduler's own charges gave 66.4 % too, so the two meters
agree on that run. It is the share the policy produced, measured
correctly, over too few slices.

Where the time goes: the guest's clock (TSC, counter) follows the host's
real time under TCG. When the host does not run a vCPU thread, the timer
interrupt that ends a slice (a cell is preempted at its second tick) is
delivered late, and the slice is charged as long as
it lasted. That is D23's rule ("the kernel charges the time from taking
a cell to giving it back"). The guest has no steal clock to tell the
host's time apart, so the time is charged to the cell that held the CPU.

On the host, an exploration of `Fair` (scratch, not committed) ran 3,000
random cases per row: slices of random length, 1 to 4 queues, one CPU
stepping. It gives the worst distance from the weight's part, in longest
slices so far:

| Domains | Slices up to 1 nominal | up to 2 | up to 20 |
|---|---|---|---|
| 2 | 1.000 | 0.999 | 0.999 |
| 3 | 0.991 | 1.126 | 1.578 |
| 4 | 0.959 | 1.260 | 1.906 |

For two domains, the case of the failing test, the policy holds within
one slice whatever the slices' lengths. The rows for three and more are
a separate finding (proposal, out of scope).

## D1. Measure as charged, over enough slices

The test reads the scheduler's charge of each root (`Host::charged_ns`),
not wall time and not the cells' quota meters. The quota meter starts
when the cell enters user mode and stops at its first exit. The charge
also counts the kernel's part of the slice, from the take to the give
back. Under steal the two meters can differ by tens of milliseconds
(one run: 736 against 701 ms). Only the charge is what the policy
balances.

`Fair::charged_ns(key)` is a cell's virtual time times its share: the
reciprocal `inv` is 2^64 / share and the virtual time is
Σ ns · inv / 2^32, so the charge is vtime · 2^32 / inv. For a cell alone
in its domain (share `WHOLE`, `inv` = 2^32 exactly) it is the exact sum
of its charges, and a domain whose lane is empty moves only the domain's
virtual time on arrival, never the cell's. It costs nothing at run time:
it reads what the charge already keeps. Rejected: a counter of charged
nanoseconds per cell or per CPU, which adds work to every switch
(`cell.switch_domains_insns` is gated), or one compiled only into test
builds, which puts a `cfg` on the switch path.

The bound follows from D23. With two busy domains each is within one
slice L of its part of the total T, so |a − ¾T| ≤ L. If T ≥ 20 L, the
share a / T is within 5 % of 75 %, and the truncated percentage is in
70..=80. So the test steps for at least 1 s, as before, and until
T ≥ 20 × the longest slice charged so far (each step runs one slice, so
the change in the sum of the two charges over a step is that slice). On
an idle host, 20 slices take about 400 ms and the test still lasts 1 s.
Under the 10-loop load the scratch copy of the new test lasted up to
2.7 s (longest slice 131 ms), and its 25 runs gave 73.1 to 75.4 %.

Rejected:

- Widening the bound. A wider band of wall time stays wrong for some
  load, and on an idle host it would no longer test the policy.
- Counting slices. The policy shares time, not turns; stretched slices
  have unequal lengths.
- A fixed number of slices. It bounds discretization but not a long
  slice near the end; the longest slice is what sets the error.
