# Tasks

## 1. The agent

- [x] 1.1 `run_python` shows a last expression's value (D1), checked by the agent's self-check; verify `python3 cells/python/agent_chat.py` (no arguments) exits with 0
- [x] 1.2 A tool call written as text runs as a call (D2), checked by the same self-check; verify as 1.1

## 2. The chat

- [x] 2.1 `rhizo chat` reads lines on a thread and says when one is queued, one prompt per line (D3); verify `cargo test -p rhizo-cli chat`

## 3. Tests

- [x] 3.1 The fake model's "square root" and "as text" answers; verify `cargo test -p xtask hostnet`
- [x] 3.2 `CHAT_MESSAGES` and `CHAT_TRANSCRIPT` extended; the native chat check in `xtask/src/differential.rs`; verify `cargo xtask --arch x86_64 test --only differential` with CPython built
- [x] 3.3 The `control` step types the two new messages at once and checks the queued note, the tool result `12.0` and the transcript; verify `cargo xtask --arch x86_64 test --only control` with CPython built
- [ ] 3.4 The same on AArch64; verify CI's AArch64 job
- [x] 3.5 Verify `cargo xtask check`, `cargo xtask --arch x86_64 test` (CPython built) and `openspec validate fix-chat-tools --strict`
