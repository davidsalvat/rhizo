## ADDED Requirements

### Requirement: Timed waits end without a timer storm
A CPU that waits for a deadline with its timer SHALL go back to its
periodic tick without its timer ever being periodic with a period shorter
than the tick, whatever the length of the wait, so that a VMM emulating
the timer is never asked to fire it faster than the tick.

#### Scenario: Back to the tick after any wait
- **WHEN** the register writes of a wait of 0 ns, 16 ns, 10 ms or any other length, then of the return to the tick, are applied in order
- **THEN** after each write the timer is one-shot, masked, or periodic with the tick's count, and after the last it is periodic with the tick's count
