# Design

## What "delivery" measures

`rhizo chat` (`cli/src/chat.rs`, `turn`) starts an `Instant` before it
sends the `Input` request and stops it when the agent's line
`\x1eturn model_ns=M agent_ns=A` arrives. The agent
(`cells/python/agent_chat.py`) takes `A` from `time.perf_counter_ns()`
between reading the user's line and printing that line, and `M` as the
sum of its model requests (each request sent until its answer is read).
`turn_line` prints `rhizo = total - M`, `agent in the cell = A - M` and
`delivery = total - A`: the input's way in (serial line, control kernel,
the cell's `read`) and the turn line's way out. `M` and `A` come from the
cell's clock, `total` from the host's; any rate difference between the
two clocks over the turn ends up in "delivery".

## Root cause

Measured on QEMU TCG, x86_64 `microvm`, 4 vCPUs, host TSC 2,100.000 MHz
(host `rdtsc` against `CLOCK_MONOTONIC` over 2 s):

- Cells' clock against the host's, with no traffic: 59.77 s of the cell's
  clock per 60.12 s of the host's (0.6% slow).
- `tsc_hz` (10 ms of the APIC timer): 2,106.3–2,112.3 MHz over ten boots.
- The same window with each end taken as the tightest of eight readings
  between two TSC reads: 2,101.2–2,101.7 MHz; with a first batch of
  readings thrown away: 2,099.99–2,100.09 MHz. The first batch's tightest
  reading is 33,000–43,000 cycles wide, the later ones 400–1,100.
- Chat turns, fake model holding its answer 65 s (its log: 65.0 s): the
  agent reads 64,803 and 64,799 ms; delivery 207.0 and 205.7 ms. With a
  3 s model: 14.0 and 10.5 ms.

Under TCG, code runs only once translated, and translation happens the
first time a block runs. In `tsc_hz`, the block after the polling loop
(the closing `rdtsc`) is translated after the last read of the APIC count
and before the TSC is read; the first reads of the count are also slow.
Those tens of microseconds count as TSC cycles of the 10 ms window: the
frequency comes out 0.3–1.5% high (the spread follows the host's load and
speed), and `cycles_to_ns` turns every interval into that much less. The
PIT path (`q35`) has the same shape and reads 0.18% slow in the in-kernel
test.

Ruled out: the network (the cell's own reading of the model's 65 s is
short, which no delay after the answer can cause), the serial line and
the CLI (the gap exists between two idle readings of the clock), ARP
expiry and tickless idle (they would add time to the host's figure, not
remove it from the cell's).

## D1. Tight readings of both clocks

`tsc_hz_over(counts)` reads each end of the window as the tightest of
eight `(rdtsc, APIC count, rdtsc)` triples, timed at the midpoint of its
two TSC reads, in a function never inlined so both ends run the same
code; one batch is read and thrown away first, which translates that
code. On KVM the readings are already tight and the result is the same.
`tsc_hz` is `tsc_hz_over(10_000_000)` (10 ms, as before).

`pit_frequency` cannot read the PIT's count around TSC reads without more
code (latching channel 2); it runs its measurement once on 100 ticks
(84 µs) before the 10 ms count, in a function never inlined, so the second
run is translated code.

Lost:

- A longer window (50 ms): divides the error by five but keeps it, and
  costs 40 ms on the first conversion.
- Preferring the APIC timing to the PIT: the PIT is the reference that
  does not assume a 1 GHz APIC timer.
- Correcting the chat's figures on the host: every interval a cell
  measures was short, not only the chat's.

## D2. The test

An x86_64 in-kernel test measures 50 ms of the APIC timer with
`tsc_hz_over` and converts that many cycles with `cycles_to_ns`: one
second within 0.05% (500 ppm). Under TCG, the old calibration fails it
(989.7 ms per second on `microvm`, 998.2 ms on `q35`); the new one passes
on both, and on KVM it compares KVM's clock with the APIC timer.

A benchmark of a 65 s turn would add minutes to `cargo xtask bench` to
show a figure the in-kernel test already pins down; the calibration is
not a benchmarked path (it runs once, lazily, and on KVM not at all).
