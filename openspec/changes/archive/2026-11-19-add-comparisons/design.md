# Design

## Decisions

### D1. What is compared with what

| Cell operation | Firecracker | Linux process |
|---|---|---|
| spawn and run `hello` to exit (`cell.run_us`) | boot a Linux guest to its first output | `posix_spawn` + wait of the same binary |
| lazy resume until runnable (`cell.resume_us`) | new VMM process + snapshot load + resume | — |
| fork of 50 MiB (`cell.fork_us`) | restore of a snapshot (one clone) | `fork` of a 50 MiB process + exit + wait |
| request served from a warm template (`workload.warm_us`) | restore of a warm snapshot | — |

Cell figures are taken with Rhizo running inside Firecracker, so the cells
pay the same virtualization as the guest they are compared with.

### D2. Firecracker driven through its API

Snapshots need Firecracker's API. `xtask` speaks HTTP/1.1 over the unix
socket itself (a request, the status line): no HTTP client dependency
(rule 10). The guest is the existing reference Linux with the minimal
`/init`; it is booted with a configuration file, paused and snapshotted
(`Full`), then restored by a fresh process per run. The first restore warms
the page cache and is dropped, like the first boot.

### D3. Honest presentation

The table states what each column measures and that processes isolate far
less. A column that could not be measured shows "—", never a quoted figure.
