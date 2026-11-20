# Design

## D1. Same program, same machine

`runsc run` of an OCI bundle holding the cell benchmark's static `hello`
starts a fresh sandbox (no network) and waits for its exit: the closest
analogue of `cell.run_us`. The restore uses an OCI bundle whose root holds the static
`idle` cell: run it detached, checkpoint it once, then time `runsc restore`
of new containers from that image until each reports `running`, and delete
each. Both need root; CI runs them with `sudo`.

## D2. Installing gVisor

`runsc` is not a pinned download in `xtask/src/tools.rs`: CI installs gVisor
release `20240603.0` before the benchmark, checked with `sha512sum -c`
against its published checksum (Ubuntu's own `runsc`, a 2023 build, failed
on the runner), and `xtask` finds it on `PATH` or through `RHIZO_RUNSC`. It
runs on gVisor's default platform (systrap) unless `RHIZO_GVISOR_PLATFORM`
names another. The measured version is recorded with the results.
