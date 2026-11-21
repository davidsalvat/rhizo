# Tasks

## 1. Protocol

- [x] 1.1 `crates/rhizo-control`: frames, decoder, requests and messages, checked decoding; host and property tests (round trips between text, any bytes, damaged frames, any payload); verify `cargo test -p rhizo-control`
- [x] 1.2 Rows in `xtask/src/deps.rs` and the AGENTS.md code map; verify `cargo xtask check`

## 2. Control kernel

- [x] 2.1 Kernel feature `cells` (set by `ktest`, `bench-cell`, `control`) and `control`; UART receive side behind `arch::console::control` (16550 port and MMIO, PL011); verify clippy of the kernel with every feature on both architectures
- [x] 2.2 `Host::look_every`, `take_output`, `spawn_with_env`, `has_finished`; `find_image`; `kernel/src/control.rs` (table, requests, output streaming); verify `cargo xtask --arch x86_64 test --only control`
- [x] 2.3 The same on AArch64; verify `cargo xtask --arch aarch64 test --only control` (CI's AArch64 job)

## 3. rhizo

- [x] 3.1 `xtask` as a library with a thin binary; `RunOptions::control` (serial console on every VMM); `xtask/src/control.rs` (`Vm`); verify `cargo test -p xtask`
- [x] 3.2 `cli/` (`rhizo-cli`, binary `rhizo`): arguments, state, server, client, templates; verify `cargo test -p rhizo-cli` and a manual `rhizo up`, `run`, `ps`, `suspend`, `resume`, `fork`, `kill`, `stats`, `down` under QEMU
- [x] 3.3 `rhizo run python-agent`, the agent waiting for `/branch`; `cargo xtask demo agent` as a wrapper, `bench=agent-demo` removed; verify a live run against a stand-in model and `cargo xtask test --only ktest` (the in-kernel agent test with templates)

## 4. Tests and metrics

- [x] 4.1 `control` step of `cargo xtask test`; verify `cargo xtask --arch x86_64 test --only control` with CPython built
- [x] 4.2 `control.*` metrics in `cargo xtask bench`, documented in `bench/README.md`; the `ci` baseline gains them from a CI run; verify the CI benchmark job
- [x] 4.3 `docs/demo.md`, README "Try it"; verify `openspec validate add-control-plane --strict`
