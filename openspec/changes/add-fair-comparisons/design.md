# Design

## D1. Linux processes inside the Firecracker guest

`bench/host/linux_process.c` is built statically with the system gcc and
packed as `/init` of an initramfs, next to the same static `hello` cell as
`/hello`, for the reference Linux kernel already used by the Firecracker
comparisons. Running as process 1, it measures, prints its result line on
the console and pauses; `xtask` reads the line and stops the VM. Three
boots, each the median of 51 runs, like the host baseline. The guest has
one vCPU and 256 MiB, which leaves room for 50 MiB copied on write.

Pairs, chosen so both sides time the same span:

| Cell | Linux process |
|---|---|
| `cell.fork_us`: fork of a frozen 50 MiB cell | `process.fork_call_us`: parent's `fork()` call |
| `cell.fork_touch_us`: fork, child writes every page, exits | `process.fork_touch_us`: fork, child writes every page, `_exit`, parent `waitpid` |
| `cell.run_us`: spawn `hello` to its exit | `process.spawn_us`: `posix_spawn` `hello`, `waitpid` |

## D2. CRIU

CRIU (`criu dump` / `criu restore`) is the established checkpoint and
restore of a Linux process. It is run as an external tool (GPL; nothing is
copied). The target is a small static program that writes as much memory
as the cell snapshot holds (8 MiB), prints a line, and sleeps. It is dumped
once; each run times `criu restore -d` until the restored process runs,
then kills it. Needs root (CI uses `sudo`). CI installs CRIU from Ubuntu's
archive or the CRIU project's PPA; when it is missing or fails, the
comparison is skipped with the reason. It runs on the host, not inside the
guest: one less layer of virtualization than the cell, which favors CRIU.

## D3. README

`docs/img/gen.py` draws the fair pairs first and the VM/sandbox tier
second, each with its own speed-up label computed against its own
baseline. Resume shows the lazy, eager and lazy-plus-touch figures; the
headline speed-up for resume uses the eager figure, against CRIU when it
was measured.
