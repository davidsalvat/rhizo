# Spec Delta

## MODIFIED Requirements

### Requirement: Idle CPUs use no time
A CPU with nothing to do SHALL wait for an interrupt without a periodic
tick, and SHALL resume when another CPU hands it work and sends it a
wake-up interrupt, without missing a wake-up sent just before it waits.
Of the idle CPUs, one at most SHALL program its timer, for the earliest
deadline of the cells that are not running; the others SHALL wait for a
wake-up interrupt only. While a CPU idles, an idle CPU's timer SHALL cover
the earliest deadline: a CPU that takes work while none does SHALL wake an
idle CPU to take that deadline over.

#### Scenario: Work for an idle CPU
- **WHEN** the boot CPU hands a function to an idle CPU
- **THEN** the idle CPU runs it and the boot CPU gets its result

#### Scenario: One cell sleeping
- **WHEN** one cell sleeps 300 ms while every other CPU of 4 vCPUs idles
- **THEN** the CPUs leave their waits at most a few times each, not at every slice, and the cell ends within 400 ms

#### Scenario: The CPU holding the timer takes work
- **WHEN** the idle CPU whose timer covers the earliest deadline takes work while another CPU idles
- **THEN** the other CPU is woken and programs its timer for that deadline
