# Proposal

## Why

Cells run only static executables. Most programs an agent runs come from a
distribution and are dynamically linked: `ls`, `sort`, `python3` name an
interpreter (`PT_INTERP`, glibc's `ld.so`), which maps the shared libraries
they need with `mmap` of files and lays them out with `MAP_FIXED`. Rhizo
refuses such executables, and its `mmap` knows neither files nor
`MAP_FIXED`. Recompiling every program statically is not an option for
"unmodified binaries from a distribution" (roadmap C.1).

## What Changes

- **Executables from the cell's files**: the host starts a cell from a path
  in the cell's own file system, with `argv` and `envp`. A dynamically
  linked executable's interpreter is loaded from the same files; the
  auxiliary vector gains `AT_BASE`; the program starts at the interpreter's
  entry. Position-independent executables load at a fixed base (no
  randomness in the kernel).
- **File pages shared by content**: the executable, its interpreter and
  every library `ld.so` maps are mapped from the file's pages, kept in
  frames by the identity of the file blocks that hold them. Every mapping
  of the same bytes, in any cell, shares one read-only frame; a write gets
  a private copy. Kept pages give way to cells under memory pressure, like
  kept executables.
- **`mmap` of files**: `MAP_PRIVATE` file mappings (read-only shared,
  copy-on-write when writable, `PROT_EXEC` under W^X), `MAP_FIXED` over
  existing mappings, `munmap` and `mprotect` on them, and `mprotect` to
  `PROT_NONE`. `MAP_SHARED` file mappings fail with `ENODEV`.
- **Start-up calls of distribution programs**: `statx`, `sysinfo`,
  `sched_getaffinity` (one CPU) and `fadvise64`.
- **Test inputs**: `xtask` packs a dynamic `hello` (a C cell linked
  without `-static`) and, natively, the runner's own `ls`, `sort` and
  `python3` (with the standard-library modules it uses and its extension
  modules), with `ld.so` and every library they need, into cell file
  systems, as it does for CPython (the distribution's programs in test
  kernels only). They are the build host's binaries, run
  unmodified as test inputs; nothing of them enters the repository.
- **Tests**: host tests of the file-page loader, the page cache, file
  mappings, `MAP_FIXED`, `PROT_NONE` and `exec`; in-kernel tests that the
  dynamic programs print what they print on Linux and that a dynamically
  linked cell suspends, resumes, forks and serves as a template; the
  differential check runs the same programs natively.
- **Metrics**: `workload.dynamic_run_us` (spawn and run the dynamic
  `hello` to its exit) and `workload.dynamic_template_us` (the same from a
  template frozen after `ld.so` relocated it), next to `cell.run_us` for a
  static `hello`.

### Out of scope

`MAP_SHARED` file mappings, symbolic links (the packed tree has none: each
file is at the path programs look for), `execve` from a cell, `vDSO`,
address-space randomization, and a Linux-process comparison of the dynamic
`hello` (the comparison guest has no dynamic loader).

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `cells`: executables from the cell's files, dynamically linked ones
  included; file pages shared between cells; dynamically linked cells
  across snapshot, fork and template.
- `linux-abi`: file mappings, fixed mappings, inaccessible pages, start-up
  calls of distribution programs.
- `workloads`: distribution programs in cells.
- `benchmarks`: dynamic linking metrics.

## Impact

`crates/rhizo-fs` (file pages), `crates/rhizo-cell` (`elf`: any ELF from a
file's first bytes, mapping from file pages, `AT_BASE`; `map_program`,
`start_at`, `PIE_BASE`; `space::map_frames`), `crates/rhizo-abi` (`exec`,
`PageCache`, `mmap`, `mprotect`, `statx`, `sysinfo`, `sched_getaffinity`,
`fadvise64`), `kernel/src/cell.rs` (`Host::spawn_file`, the kept file
pages), `kernel/src/tests.rs`, `cells/c/dynamic/hello.c`,
`cells/python/{dynamic_demo,pack_stdlib}.py`, `cells/expected.rs`,
`xtask/src/{dynamic,differential,bench}.rs`, `kernel/build.rs` (images
embedded in test kernels only), `kernel/src/arch/aarch64/boot.rs` (the
early tables map the whole image: test kernels now pass 32 MiB),
`bench/README.md`. The `bench-cell` kernel embeds the dynamic `hello`
with its interpreter and `libc` (about 2.5 MiB). No existing metric changes
by design.
