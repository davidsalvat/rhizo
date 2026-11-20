# Design

## D1. Built only from a given source

MicroPython's build runs its own Makefiles, so `xtask` builds it only when
`RHIZO_MICROPYTHON_SRC` points at a checkout of the pinned commit (checked
with `git rev-parse`). CI fetches that commit in the jobs that build cells.
Without it the kernel embeds an empty image and the MicroPython test and
differential check say they did not run; with it, a failed build fails.

## D2. The same module everywhere

`rhizo_demo.py` lives in `cells/python/`; the kernel test and the benchmark
give it to the cell as `/lib/rhizo_demo.py` (initial files, H8) and run
`micropython -c` with `lib` on `sys.path`; the differential check does the
same in an empty directory on Linux. Its output was computed with CPython
from the same file.

## D3. `select` as a sleep

MicroPython's Unix port sleeps with `select(0, NULL, NULL, NULL, &tv)`.
With no descriptors, `select` (x86_64) and `pselect6` (both) are a sleep on
the monotonic clock, through the scheduler's sleep list like `nanosleep`.
