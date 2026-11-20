# Design

## D1. A pinned release, built natively from its tarball

CPython 3.13.18 is the pinned release: the current 3.13 maintenance line,
licensed under the PSF License 2.0 (permissive, not copyleft; nothing of it
is copied into the repository, it is built from python.org's tarball).
`xtask/src/cpython.rs` holds the version and the tarball's SHA-256, checked
with `sha256sum` before unpacking.

The build is CPython's own: `configure --disable-shared --without-ensurepip
--disable-test-modules` with `LDFLAGS=-static`, then `make python
pybuilddir.txt LINKFORSHARED=` (no `-export-dynamic` in a static
executable), and `strip`. `Modules/Setup.local` lists the C modules built
into the executable; `make python` builds no shared module, so the
standard library's other C modules simply do not exist. The result is a
static glibc executable like the C cells.

It is built only natively (host architecture = cell architecture), as the
differential check runs only there: CI's x86_64 jobs build the x86_64
cell, the AArch64 job the AArch64 one. Cross-building CPython (it runs its
own interpreter during the build) is not worth it here.

A stamp file holds the recipe (version, configure options, `Setup.local`);
a build directory with another recipe is rebuilt. CI caches the build
directory per runner architecture and recipe file, saved even when a later
check fails, since a build takes minutes.

Alternatives: a distribution's `python3` (dynamic, not ours to pin);
python-build-standalone (a third-party build recipe, larger); keeping
MicroPython only (not what agents run).

## D2. The standard library as compiled modules in the cell's files

CPython computes its paths from its executable: started as
`/usr/bin/python3` with no environment, it searches the directories above
`/usr/bin` (not `/` itself) for `lib/python3.13/os.py` or `os.pyc` and finds
prefix `/usr`, then for the directory `lib/python3.13/lib-dynload` (exec
prefix `/usr`). So the cell gets `/usr/lib/python3.13/` holding the modules
and an empty `lib-dynload`, the layout of a distribution's CPython, and no
warning is printed. The executable itself is not a file in the cell: it is
the image the cell is spawned from (`readlink` and `realpath` on it fail as
on Linux for a missing file, which CPython accepts).

Only the modules used are packed: `cells/python/pack_stdlib.py`, run by the
built interpreter, runs the script once with its output discarded and
compiles each standard-library module then in `sys.modules` (start-up
included) to a sourceless `.pyc` (unchecked hash: never compared with a
source) at its relative path. CPython imports sourceless `.pyc` files from a
directory on `sys.path` directly. About 40 modules, under 1 MiB.

Why a tree of `.pyc`, not a zip: a zip would be imported through
`zipimport` (Python code that parses the archive's directory at start-up)
and still needs the directory landmark; a tree is how an installed CPython
works (`stat`, `open`, `read`, `getdents64`), costs no extra start-up work
and is the same layout on Linux, in the cell and in the Linux guest of the
comparison. Why compiled: compiling `json`, `re` and `enum` from source at
every start would dominate the cold figure, and no installed CPython does
that (it caches bytecode).

`xtask` encodes the tree (`lib/` and `app/agent_demo.py`) as a cell file
system with `rhizo-fs`'s own encoding (the files section of snapshots), so
xtask depends on `rhizo-fs` (workspace crate, MIT). The kernel embeds the
bytes (`cpython-files`, empty when not built), decodes them once with the
validating decoder and hands each CPython cell a clone: blocks are shared
copy-on-write, so a spawn copies no file data. The differential check and
the Linux guest use the same tree from disk (`target/cpython/<arch>/root`).

## D3. The workload and what is measured

`cells/python/agent_demo.py` imports `json`, `re`, `datetime`, `random`,
`math`, `struct`, `binascii`, `time` and warms up by answering one request
quietly; it prints `warm` (flushed), sleeps 1 ms (where a host may freeze
it) and answers a second request in JSON, then prints `cpython ok`. Output
does not depend on hash randomization, time zone or locale.

The command is `/usr/bin/python3 /app/agent_demo.py`, with `site` (the default
start-up). On Linux the differential check runs the same executable on the
same tree with an empty environment and requires an empty standard error,
since the cell's console is both outputs.

- `workload.cpython_cold_us`: spawn from the executable to the cell's exit,
  file system clone included (the decoded files are prepared before
  timing, as a host keeps its image in memory).
- `workload.cpython_warm_us`: spawn from the template frozen after `warm`
  (its timers expire, so the child does not finish the sleep) to its exit.

Both include interpreter finalization at exit. Medians over 7 rounds per
boot, as for MicroPython.

## D4. CPython as a Linux process in the same VM

The fair pair (as in `add-fair-comparisons`): a Linux guest in Firecracker
whose initramfs holds the same static CPython at `/usr/bin/python3`, the same
tree, the script, `bench/host/cpython_forkserver.py`, and
`bench/host/linux_cpython.c` as `/init`.

- Cold (`workload.cpython_cold_us/linux-process/firecracker`): `/init`
  forks and `execve`s `python3 /app/agent_demo.py` with an empty
  environment, reads its output from a pipe and waits for it.
- Warm (`workload.cpython_warm_us/linux-process/firecracker`): the
  pre-fork pattern of servers. `cpython_forkserver.py` imports and warms up
  `agent_demo`, then forks one child per request; the child answers and
  leaves through the interpreter's normal exit (finalization included, as
  in the cell), the parent waits. Its median is printed back to `/init`.

`initramfs_with` learns nested paths (it emits each directory before its
files) and the Firecracker boot loop is shared with the process
comparison. Skipped with the reason when CPython is not built or
Firecracker cannot run.
