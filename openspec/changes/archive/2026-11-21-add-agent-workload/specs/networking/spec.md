# Spec Delta

## ADDED Requirements

### Requirement: TCP_NODELAY on a connection through the card
Setting `TCP_NODELAY` on a TCP connection through the network card SHALL
take effect at once, whether it was set before connecting or after.

#### Scenario: Set after connecting
- **WHEN** a cell sets `TCP_NODELAY` on an established connection to the outside and writes twice before the peer acknowledges the first write
- **THEN** the card sends both writes without waiting for the acknowledgment
