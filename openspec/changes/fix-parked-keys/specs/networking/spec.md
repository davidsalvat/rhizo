## ADDED Requirements

### Requirement: Parked sockets answer only their own record
A listener or connection parked by a suspend SHALL be re-attached only by
a snapshot record carrying the key it was parked with. Keys SHALL be
derived from a secret the kernel holds and no snapshot contains, so that
no key can be guessed from the order of suspends or from other keys.

#### Scenario: Forged keys
- **WHEN** records with keys 0 to 63, and keys next to the keys of two parked connections, are imported
- **THEN** each comes back as a new socket in its record's own state, and both parked connections come back only for their own records
