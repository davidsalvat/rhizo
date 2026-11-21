# Design

## D1. A dynamically linked CPython loading manylinux wheels

Two ways to get numpy into a cell:

- (a) compile numpy's C sources into the static CPython (`Setup.local`, or
  a static build of its extension modules linked into the interpreter);
- (b) a dynamically linked CPython that opens the extension modules of the
  wheels PyPI publishes (`.so` files, `dlopen`), as every Linux CPython
  does.

(b) is chosen: it is what "pip install X in a cell" means, it runs the
binaries everyone runs (manylinux wheels, unmodified), and it adds a
package by adding a wheel, not by porting its build to CPython's. (a)
would need numpy's build (meson, generated sources, CPU dispatch), a BLAS
built statically, and the same again for every package with C code.

What (b) needs from the kernel was the open question: `dlopen` of a
shared object maps it from the file (`MAP_PRIVATE`, `MAP_FIXED` over a
reservation, `mprotect` of its RELRO, `PROT_NONE` gaps), and numpy's
libraries add thread-local storage (OpenBLAS has a 140 KiB `PT_TLS`
segment), a C++ runtime, a Fortran runtime and threads (OpenBLAS starts
a pool at load). All of it is user space or already supported by
`add-dynamic-linking`: `ld.so` handles `PT_TLS` of a library itself
(static TLS surplus or dynamic TLS), and `clone`, futexes and
`sched_getaffinity` are there. No change to the loader or the system
calls was needed: numpy and requests run on C.1's as they are, and the
tests below hold that. The one call they make that Rhizo does not
implement is `mbind` (OpenBLAS placing its buffers on a NUMA node): it
fails with `ENOSYS`, as on a Linux kernel built without NUMA, and
OpenBLAS goes on without it.

Which CPython: the distribution's `python3` is already packed by
`add-dynamic-linking`, but its version is the runner's (3.12 on Ubuntu
24.04), so the wheels' tags would depend on the host. Instead `xtask`
builds the pinned CPython 3.13.18 again from the tarball CI already
fetches, configured as the static one but without `-static` (extension
modules built as shared objects in `lib-dynload`, the C API exported by
the executable, CPython's default), and installs it (`make altinstall
DESTDIR=...`, prefix `/usr`) into a staging tree (`target/cpython/<arch>-dynamic/stage`); the
sources, objects and static library are then removed (300 MiB less in
CI's cache, which holds `target/cpython`). The static CPython cell is
untouched: its start is faster (no relocations), and its metrics stay
comparable with their history. The build adds about three minutes to a
CI run without a cache, none with one.

## D2. Wheels pinned like the tarball

`xtask/src/packages.rs` pins each wheel: its file name, its path on
`files.pythonhosted.org`, its SHA-256 and its architecture (or none for
pure Python). numpy 2.5.4 (`cp313`, `manylinux_2_28`, x86_64 and
aarch64), requests 2.34.2 and what it needs: urllib3 2.8.0, idna 3.20,
charset-normalizer 3.5.2 (its pure Python wheel), certifi 2026.10.23;
each the latest release at least two weeks old. `xtask` downloads a
wheel with `curl` into `target/cpython/wheels` once (CI's CPython cache
keeps them; its key also hashes `packages.rs`), checks its SHA-256, and
installs it into the staging tree's `site-packages` by extracting it: a
wheel is a zip archive laid out as installed (PEP 427), which is what pip
does for these wheels (none has scripts or data to relocate). The
staging interpreter extracts it (`python -I -m zipfile -e`): no new
host tool. Built only with `RHIZO_CPYTHON_SRC` and only natively, like
CPython.

Licenses (test inputs, never copied into the repository): numpy
BSD-3-Clause, with OpenBLAS (BSD-3-Clause) and libgfortran
(GPL-3.0-or-later with the GCC Runtime Library Exception) vendored in
`numpy.libs` on aarch64 and x86_64; requests Apache-2.0; urllib3 MIT;
idna BSD-3-Clause; charset-normalizer MIT; certifi MPL-2.0 (a data file of
CA certificates, unmodified). The packed tree keeps each package's
`.dist-info`, licenses included. The interpreter's other libraries come
from the build host as in `add-dynamic-linking` (`libffi` MIT,
`libstdc++` and `libgcc_s` GPL-3.0 with the runtime exception, OpenSSL
Apache-2.0, ...).

## D3. Packing: what the scripts use

The cell's files are packed as CPython's are (`add-cpython` D2):
`pack_stdlib.py`, run by the staging interpreter, runs the scripts and
packs every module they and the start-up imported. It now also takes
modules from `site-packages` (compiled to sourceless `.pyc`, extension
modules copied), the data files next to an imported package's modules
(certifi's `cacert.pem`), and whole the `<name>.libs` directories
(libraries a wheel vendors) and `<name>.dist-info` directories. The
requests script needs a server: `xtask` gives it its own fake model on
127.0.0.1, and `pack_stdlib.py` accepts a script's arguments after it,
one per line. Only what is imported is packed: numpy's tests, headers,
stubs and unused extension modules stay out (13 MiB of numpy's 33 MiB
are packed, with the 27 MiB of `numpy.libs`).

The dynamic packing (`dynamic::pack`) adds `ld.so` and the libraries
every ELF file needs from the host; a library a file's run path names
relative to the file (`$ORIGIN/../../numpy.libs`, how auditwheel vendors
OpenBLAS) and that the tree holds is used from there, not searched on
the host. The result is `packages-files`, embedded in test kernels and
in the `bench-cell` kernel.

In a cell the interpreter is `/usr/bin/python3`, started as
`usr/bin/python3` from `/`; it finds its prefix (`/usr`) by the landmark
`lib/python3.13/os.pyc` and `lib-dynload`, and `site` adds
`/usr/lib/python3.13/site-packages`, as on Linux. Natively the same
command from the top of the tree finds the tree's own files, never the
host's.

## D4. Memory: file data and VMs

numpy's files are large: OpenBLAS is 24 MiB on x86_64 and 26 MiB on
aarch64, numpy's core module 6 to 11 MiB; the packed tree is about
65 MiB (CPython, its libraries and the standard library's extension
modules, stripped of their debug information, are 20 MiB of it).

That is more than a cell could hold: its file data was bounded at
64 MiB (spec `files`). The bound becomes 128 MiB (`rhizo_fs::DATA_MAX`):
room for the interpreter, numpy and a package or two more, and still a
bound on the kernel memory a cell's files take. Files given to many
cells are shared copy-on-write, so the bound is per cell, not per copy;
memory pressure is handled as before (a cell is refused memory, the
kernel keeps running).

The tree is embedded in the kernel image, decoded into the kernel heap
when first used (`rhizo_fs`, copy-on-write blocks shared by every cell
given the files), and the mapped pages are kept in the page cache
(`add-dynamic-linking` D1): about three times its size while a numpy
cell runs. A snapshot of a cell that imported numpy is self-contained
(`add-dynamic-linking` D7): it holds the cell's files and every page it
mapped, well over 100 MiB, and the test that resumes one from its bytes
holds both at once. The test VMs (`ktest`, `ktest-smp`, and `ktest` on
q35) grow from 128 MiB to 1 GiB, and the cell benchmark VM from 256 MiB
to 512 MiB; the boot check, the self-checks and the boot benchmarks keep
128 MiB. A larger guest does not change what the cell benchmarks measure
(no metric depends on free memory; the 50 MiB fork benchmark is the one
that needed the room). The memory pressure test asked twenty cells for
16 MiB each, more than a 128 MiB VM holds; it now asks each for an
eighth of the free memory, so that twenty want more than any VM holds.
Mapping the embedded bytes directly (no decoded copy), and snapshots that
refer to file pages by content instead of copying them, would save most
of this; both are left for later.

## D5. Tests and metrics

In-kernel tests (`kernel/src/tests.rs`):

- `numpy_from_a_wheel_computes_as_on_linux`: `numpy_demo.py` (integer
  array operations, `linalg.solve` and `det`, `np.hypot`, a 64x64 matrix
  product through OpenBLAS) prints [`NUMPY_STDOUT`]; once the kept pages
  are dropped, memory is back to what it was. The output is exact or
  rounded, so it does not depend on which kernels OpenBLAS picks for the
  CPU (the host's and QEMU's differ).
- `requests_gets_from_the_host_as_on_linux`: `requests.get` of the host's
  fake model through the network card prints the status, content type,
  detected encoding (charset_normalizer) and the tool call of the JSON
  body.
- `a_cell_that_imported_numpy_suspends_resumes_and_forks`: a cell stopped
  after `import numpy` (`wait`) is suspended, resumed from the bytes,
  forked, and frozen as a template; every copy prints the computation.

The differential check runs both scripts natively from the top of the
tree with an empty environment (requests against the host's fake model)
and compares with `cells/expected.rs`.

`bench-cell` prints `rhizo-numpy cold_ns=... warm_ns=...`:

- `workload.numpy_cold_us`: spawn the packages' CPython from its files,
  `import numpy`, compute, exit; robust figure over 10 runs after a first
  that fills the page cache;
- `workload.numpy_warm_us`: the same from a template frozen after
  `import numpy` (it prints `ready`): the template case, import once and
  fork many.

Both are new: no baseline until a CI run records one.

## D6. numpy as a Linux process in the same VM

As CPython (`add-cpython` D4): a Linux guest in Firecracker whose
initramfs holds the packages cell's tree and `bench/host/linux_numpy.c`
as `/init`. Cold: fork and exec `python3 /app/numpy_demo.py` until it
exits, median of 7; warm: `bench/host/numpy_forkserver.py`, a CPython
process that imported numpy and forks one child per request, which runs
the script's computation and exits. They report `workload.numpy_cold_us`
and `workload.numpy_warm_us` for guest `linux-process`, VMM
`firecracker`.

## D7. Still missing for "pip install X in a cell"

- pip itself: `ensurepip` is disabled in both CPython builds; pip needs
  `subprocess` (`fork`/`exec` from a cell), which cells do not have.
- HTTPS to PyPI: `_ssl` loads (OpenSSL from the host is packed when the
  host has it), but a TLS client also needs certificate verification
  against the clock and the cells' entropy for keys; untested.
- Wheels with console scripts or data directories (install schemes), and
  source distributions (a compiler in a cell).
- The page cache holds at most 64 MiB of file pages: two large packages
  (numpy and, say, pandas) would start evicting each other.
