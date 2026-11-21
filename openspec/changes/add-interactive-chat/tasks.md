# Tasks

## 1. Standard input from the host

- [x] 1.1 `rhizo-fs`: the host's input pipe (`host_input`, `push_input`, `close_input`, `has_input`), its encoding and consistency check; snapshot format 7; host tests (whole or nothing, end of file, closed by the cell, fork copies, snapshot round trip, corrupt index); verify `cargo test -p rhizo-fs -p rhizo-snapshot`
- [x] 1.2 `rhizo-control`: version 2, `Input`, `CloseInput`, `Run::input`, `MAX_INPUT`; round trips, bounds and property tests over the new requests; verify `cargo test -p rhizo-control`
- [x] 1.3 Kernel: `Host::input` (write or close, wake descriptor waiters), `Host::has_output`, the requests and prompt output in `src/control.rs`; verify `cargo xtask check` (clippy of the kernel on both architectures)

## 2. The chat

- [x] 2.1 `cells/python/agent_chat.py` (tools, limits checked when packed, markers), packed with the CPython cell; `Model.chat` takes the tools; verify the CPython build and `cargo xtask --arch x86_64 test --only control` (the scripted agent's transcript unchanged)
- [x] 2.2 The fake model's chat answers; verify `cargo test -p xtask hostnet`
- [x] 2.3 `rhizo chat` (`cli/src/chat.rs`, `args.rs`); verify `cargo test -p rhizo-cli`

## 3. Tests, metric, docs

- [x] 3.1 The conversation in the `control` step, `CHAT_MESSAGES` and `CHAT_TRANSCRIPT` in `cells/expected.rs`; verify `cargo xtask --arch x86_64 test --only control` with CPython built
- [x] 3.2 The same on AArch64; verify CI's AArch64 job
- [x] 3.3 `chat.turn_us` in `cargo xtask bench`, documented in `bench/README.md`; verify `cargo xtask --arch x86_64 bench --vmm qemu-microvm --runs 1 --no-reference` lists it
- [x] 3.4 "Talk to an agent" in `docs/demo.md`; verify `openspec validate add-interactive-chat --strict`
