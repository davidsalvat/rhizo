## ADDED Requirements

### Requirement: No cell reads another through shared counters
What the kernel tells a cell about memory (`sysinfo`) and the ephemeral
ports it gives a cell SHALL NOT depend on what other cells allocate or
bind, and no cell SHALL make the kernel drop the kept file pages another
cell maps (findings S24 to S26 in `docs/security.md`). Ports other cells
hold, pages other cells keep in the cache, cell identifiers and errors
from pools all cells share are outside this requirement (change
`add-cell-hierarchy`).

#### Scenario: Free memory
- **WHEN** a cell with a 64 MiB memory quota reads `sysinfo`, then again while another cell holds 32 MiB
- **THEN** it reads 64 MiB of total memory both times and the same free memory

#### Scenario: Ports
- **WHEN** another cell binds 100 sockets to ephemeral ports and closes them between two binds of a cell to port 0
- **THEN** the cell's two ports are one apart

#### Scenario: Page cache
- **WHEN** a cell takes memory until refused while another cell maps a page of its own file
- **THEN** that page stays kept
