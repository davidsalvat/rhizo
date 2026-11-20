# Design

## D1. Same program, same machine

`runsc do` runs the cell benchmark's static `hello` in a fresh sandbox
(default platform, no network) and waits for its exit: the closest analogue
of `cell.run_us`. The restore uses an OCI bundle whose root holds the static
`idle` cell: run it detached, checkpoint it once, then time `runsc restore`
of new containers from that image until each reports `running`, and delete
each. Both need root; CI runs them with `sudo`.

## D2. Installing gVisor

`runsc` is not a pinned download in `xtask/src/tools.rs`: CI installs it
before the benchmark (the Ubuntu `runsc` package from the signed archive,
else release `20240603.0` checked with `sha512sum -c` against its published
checksum), and `xtask` finds it on `PATH` or through `RHIZO_RUNSC`. The
measured version is recorded with the results.
