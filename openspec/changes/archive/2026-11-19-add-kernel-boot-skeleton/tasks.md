# Tasks

## 1. Workspace and toolchain

- [x] 1.1 Create the Cargo workspace (`kernel/`, `crates/rhizo-hal/`, `crates/rhizo-core/`, `xtask/`) and `rust-toolchain.toml` pinning a stable toolchain with the `x86_64-unknown-none` target, `rustfmt` and `clippy`; verify `cargo metadata` lists all four members
- [x] 1.2 Add the `cargo xtask` alias in `.cargo/config.toml`; verify `cargo xtask --help` prints the `build`, `run` and `test` subcommands

## 2. Hardware interfaces and core logic

- [x] 2.1 Define the `Console`, `Power` and `CycleCounter` traits in `rhizo-hal` with in-memory test doubles; verify a host test writes through the fake console and reads back the exact bytes
- [x] 2.2 Implement the boot banner and boot report (version, boot duration in cycles, boot-complete message) in `rhizo-core` against the traits; verify host tests check the exact output lines
- [x] 2.3 Implement the in-kernel test runner logic in `rhizo-core` (iterate cases, print name and `ok`, return the final status that the kernel passes to `Power`); verify host tests cover the all-pass path, the returned status and that a failing case is named before it runs

## 3. Bootable kernel

- [x] 3.1 Write the linker script and PVH entry note so the image loads at 1 MiB; verify with `readelf -n` that the image carries the PVH entry note
- [x] 3.2 Write the 32-bit to 64-bit trampoline (start-info magic check, identity-mapped page tables, long mode, stack, jump to Rust); verify `cargo xtask run` reaches Rust code by printing the banner, and an in-kernel test reads the PVH magic through the recorded start-info pointer (the wrong-magic halt path cannot be triggered through QEMU's PVH loader and is verified by review)
- [x] 3.3 Implement the x86_64 hardware interfaces (16550 serial on COM1, `isa-debug-exit`, `rdtsc`); verify `cargo xtask run` prints the banner, the boot time and the boot-complete line, and exits with status 0
- [x] 3.4 Implement the panic handler (message and location on serial, failure exit); verify with a temporary in-kernel test that panics that the run shows the message and file/line and exits non-zero, then keep it as a negative test driven by `xtask`

## 4. Developer tooling

- [x] 4.1 Implement `cargo xtask build` and `cargo xtask run` (QEMU flags from design D7, KVM when available, `--gdb` option); verify both commands on a fresh checkout
- [x] 4.2 Implement `cargo xtask test` (host tests, then the `ktest` kernel in QEMU, exit-status mapping from design D6, timeout with `RHIZO_TEST_TIMEOUT`); verify passing, failing and hanging kernels each produce the expected exit status and message
- [x] 4.3 Add initial in-kernel tests (serial output works, cycle counter is monotonic); verify `cargo xtask test` prints each test name with `ok` and exits 0

## 5. Continuous integration and documentation

- [x] 5.1 Add the GitHub Actions workflow (fmt, clippy with warnings denied for host crates and the kernel target, `cargo xtask test`); verify it passes on the pull request
- [x] 5.2 Document prerequisites and the build, run and test commands in `README.md` and `AGENTS.md`; verify the documented commands run as written on a fresh checkout, and record the QEMU version used by CI

## Workflow follow-up

- Archive this change with `openspec archive add-kernel-boot-skeleton` after it is merged.
- Mark milestone H0 as done in the README roadmap.
