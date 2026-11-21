# Tasks

## 1. Library

- [x] 1.1 `rhizo_fs::Page`, `Tables::file`, `file_at` and `page` (D1): host test that a held page keeps its bytes and key while the file is written, forked and truncated; verify `cargo test -p rhizo-fs pages_name`
- [x] 1.2 `elf::Image::parse_file`, `interpreter`, `map_file` and `Aux` with `AT_BASE`; `space::map_frames`; `map_program`, `start_at`, `PIE_BASE` (D2, D3); host tests of parsing from the first bytes and of a program and its interpreter mapped from file pages (shared, copy-on-write, `.bss` page private, every frame back when memory runs out); verify `cargo test -p rhizo-cell`
- [x] 1.3 `rhizo_abi::PageCache` and `FilePages` (D1), `exec` (D2); `mmap` of files and `MAP_FIXED`, `mprotect` to `PROT_NONE` (D4), `MAP_SHARED` of files refused (D6), `statx`, `sysinfo`, `sched_getaffinity`, `fadvise64`; host tests of the cache, of a dynamic loader's sequence of calls, of `exec` and its errors, and the fuzzer with a file to map; verify `cargo test -p rhizo-abi`

## 2. Kernel

- [x] 2.1 `Host::spawn_file`, the kept file pages (`PAGES`, `KernelPages`: instruction cache made coherent once per page) dropped with the kept executables, `Services::memory`; verify `cargo xtask check`
- [x] 2.2 In-kernel tests `dynamically_linked_programs_behave_as_on_linux` and `a_dynamically_linked_cell_suspends_resumes_and_forks`; verify `cargo xtask test` and `cargo xtask test --only ktest-smp` on both architectures

## 3. Test inputs

- [x] 3.1 `cells/c/dynamic/hello.c`, `cells/python/dynamic_demo.py`, `pack_stdlib.py` copying extension modules and `os`, `xtask/src/dynamic.rs` packing `dynamic-files` and `distro-files` (D5, the latter in test kernels only), the AArch64 boot tables covering images past 32 MiB, `DYNAMIC` in `cells/expected.rs` and the differential check running them natively; verify `cargo xtask test --only differential` on x86_64 and `cargo xtask test --only ktest` on both architectures

## 4. Benchmarks and documentation

- [x] 4.1 `rhizo-dynamic` line of the `bench-cell` kernel and `workload.dynamic_run_us`, `workload.dynamic_template_us` in `xtask/src/bench.rs` with a parser test; the dynamic `hello` as a Linux process (`linux_process.c`, in the Firecracker guest and on the host) with a parser test; `bench/README.md` describes them (D8); verify `cargo test -p xtask` and the CI benchmark gate passes with both metrics present and no metric worse
- [x] 4.2 `AGENTS.md` code map names the new parts of `rhizo-cell`, `rhizo-abi` and `xtask`; verify `openspec validate add-dynamic-linking --strict`
