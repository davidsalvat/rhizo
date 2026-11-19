# Proposal

## Why

Cells have no files: `open`, `read` and `stat` of a path fail. Agents write
code, read data and import modules; interpreters (MicroPython, the workload
the roadmap names) load their modules from files. Rhizo's rule 6 says every
piece of per-cell state is snapshotted and cloned with the cell, so files
must live inside the cell and travel with suspend, resume and fork. That is
also the point: forking a cell clones its whole environment, files included,
in microseconds, which no layer on top of a shared file system offers.

## What Changes

- New crate `rhizo-fs`: an in-memory file system per cell (directories,
  regular files, an inode table and open-file descriptions as plain data),
  shared copy-on-write between forked cells, encodable to bytes; host
  tests including a property test against a model.
- System calls on files with Linux semantics: `openat` (and `open` on
  x86_64), `close`, `read`, `write`/`writev` on files, `pread64`,
  `pwrite64`, `lseek`, `fstat`/`newfstatat` (and `stat`/`lstat` on x86_64),
  `faccessat`, `getdents64`, `mkdirat`/`mkdir`, `unlinkat`/`unlink`/`rmdir`,
  `renameat`/`renameat2`/`rename`, `getcwd`, `chdir`, `ftruncate`, `fsync`,
  `dup`, `dup2`, `dup3`, `fcntl` (duplicating and file status flags).
- Snapshots carry the file system (format version 3); forks and templates
  share it copy-on-write.
- The host can give a cell initial files when it spawns it.
- Benchmarks: `cell.file_write_read_us`, `cell.fork_with_files_us`.
- The bench gate measures robustly: the cell figures of a run are the best
  of several rounds, so one slow round on a shared runner does not decide.

### Out of scope

Persistence to a disk, sharing files between live cells, symbolic and hard
links, permissions and ownership beyond fixed modes, file locks, `mmap` of
files, pipes and sockets.

## Capabilities

### New Capabilities

- `files`: the per-cell file system.

### Modified Capabilities

- `linux-abi`: file system calls.
- `snapshots`: files in snapshots.
- `fork`: files shared copy-on-write.
- `benchmarks`: file metrics and robust cell figures.
