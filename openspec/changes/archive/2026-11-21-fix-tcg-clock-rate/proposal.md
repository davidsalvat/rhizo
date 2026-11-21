# Proposal

## Why

On a VPS without KVM (QEMU TCG, x86_64 `microvm`, 4 vCPUs), `rhizo chat`
against a model held for 65 s by a host proxy reported, for that turn,
`model 64,322 ms, rhizo 714.7 ms (agent in the cell 30.7 ms, delivery
684.1 ms)`, and 931.7 ms of "delivery" on another 62.6 s turn, against
39–117 ms on turns of 3–7 s. The delivery looked slow after a long
silence.

It is not. "Delivery" is the host's round trip (`Instant`, from the
`Input` request to the agent's turn line) minus the agent's own time for
the turn (`agent_ns`, read by CPython's `time.perf_counter_ns` inside the
cell, i.e. `clock_gettime(CLOCK_MONOTONIC)`). Without KVM, that clock runs
slow: the x86_64 kernel finds the TSC frequency by timing 10 ms of the
local APIC timer (`microvm` has no PIT) or of the PIT (`q35`), and under
emulation the code after the timing loop is translated between the last
read of the reference timer and the closing read of the TSC. The extra
cycles put the frequency found 0.3 to 1.5% too high (2,106–2,112 MHz
for a 2,100.000 MHz TSC on this host; more on a slower VPS), so every
interval a cell measures is that much short. A 65 s model call then reads
as 64.3–64.8 s, and the missing 0.2–0.7 s lands in "delivery", which grows
with the length of the turn, not with anything the turn does:

| Turn (this host, TCG, 4 vCPUs) | model (cell's clock) | delivery |
|---|---|---|
| model held 65 s, before | 64,803 / 64,799 ms | 207.0 / 205.7 ms |
| model held 3 s, before | 3,068 / 2,993 ms | 14.0 / 10.5 ms |

The other hypotheses are ruled out by the same runs: the model's own
reading of 65 s is short although the fake model's log shows it held the
answer 65.0 s, which no network, serial-line or CLI delay can do; and the
host's clock against the cell's over 60 s of idleness, with no traffic at
all, shows the same 0.6% gap.

## What Changes

- **Each end of the calibration is a tight reading.** The APIC timing
  takes each end as the tightest of eight readings of the timer between
  two TSC reads, timed at their midpoint, after a first batch that only
  gets the code translated. The PIT timing runs once on a short count
  before the 10 ms one, so both run translated code.
- **An in-kernel test** checks that the TSC's conversion to time, however
  it was found, agrees within 0.05% with 50 ms of the APIC timer.

After the change, on the same host: the 65 s model reads 65,004–65,019 ms
and its delivery 1.5–8.8 ms, as for 3 s turns (2.4–4.6 ms); one second
of the APIC timer reads within 30 ppm.

## Out of scope

- AArch64: its counter frequency is the architectural `CNTFRQ_EL0`,
  not measured.
- KVM (QEMU with KVM, Firecracker, Cloud Hypervisor): the frequency comes
  from KVM's clock, unchanged.
- The chat's figures: they are right once the cell's clock is.

## Capabilities

- `linux-abi`: the monotonic clock cells read follows real time.

## Impact

- `kernel/src/arch/x86_64/irq.rs` (`tsc_hz`, `tsc_hz_over`),
  `kernel/src/arch/x86_64/timer.rs` (`pit_frequency`), and an x86_64
  in-kernel test. The calibration runs once, lazily; it takes two extra
  batches of eight timer reads (APIC) or a 84 µs PIT count more. No
  benchmarked path changes.
