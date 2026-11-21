## ADDED Requirements

### Requirement: Card interrupts
A virtio-mmio network card whose interrupt the platform names SHALL
interrupt the boot CPU when it hands over received frames, and the kernel
SHALL then poll it; the kernel SHALL NOT poll such a card on a fixed
period. A card without a routed interrupt SHALL be polled.

#### Scenario: Round trips with the CPU asleep
- **WHEN** a cell exchanges 20 messages with the host's echo server through a virtio-mmio card and nothing else runs
- **THEN** the replies arrive and the boot CPU returns from waiting at most 4 times per round trip
