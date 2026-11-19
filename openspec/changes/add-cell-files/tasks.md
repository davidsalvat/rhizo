# Tasks

## 1. File system

- [x] 1.1 Create `rhizo-fs`: tree, files in shared blocks, descriptions and descriptors, paths, errors; host tests and a property test against a model with forks; add it to the dependency table
- [x] 1.2 Encode and decode the file system (D3); host tests: round trip, truncation, bad references

## 2. System calls and kernel

- [x] 2.1 `rhizo-abi`: the file calls of `linux-abi` through the cell's file system; host tests
- [x] 2.2 Kernel: a file system per cell; fork, freeze/templates, suspend/resume (snapshot format version 3) carry it; initial files at spawn
- [x] 2.3 Test cells (`libc_files` C program, differential on Linux) and in-kernel tests: Linux behavior, privacy, suspend/resume with an open file, fork divergence, template files, memory returned

## 3. Benchmarks and documentation

- [x] 3.1 `cell.file_write_read_us`, `cell.fork_with_files_us`; cell figures as the minimum of per-round medians (D4)
- [x] 3.2 README roadmap (H8), AGENTS.md code map, `bench/README.md`, `cargo xtask ci` on both architectures
