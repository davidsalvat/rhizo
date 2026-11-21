## ADDED Requirements

### Requirement: Connections to the outside wait for silent peers
A TCP connection through the card SHALL give up on its peer (and report
`ECONNRESET`) only while it is being opened or while bytes it sent wait
for their acknowledgment, after 20 s without an answer. An open
connection with nothing outstanding SHALL stay open however long its peer
stays silent. A connection the cells closed SHALL be aborted only if its
peer has not finished closing after 60 s.

#### Scenario: A model that thinks for 45 s
- **WHEN** a cell sends a request over a connection to the outside, the peer acknowledges it and answers 45 s later
- **THEN** the cell receives the answer on the same connection

#### Scenario: An idle kept-alive connection
- **WHEN** a connection to the outside carries nothing for ten minutes and the cell then sends on it
- **THEN** the peer's reply arrives on the same connection

#### Scenario: A peer that vanishes
- **WHEN** a cell sends bytes to a peer that no longer answers
- **THEN** the connection reports `ECONNRESET` after the 20 s timeout

#### Scenario: Reconnecting
- **WHEN** a connection to the outside is reset by its peer, or its peer closes it between two requests, and the cell opens a new one
- **THEN** the new connection is open and carries an echo within a second
