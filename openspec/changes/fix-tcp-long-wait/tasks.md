# Tasks

## 1. Reproduce

- [x] 1.1 Host test with a `smoltcp` peer that answers after 45 s, failing with `ECONNRESET` 20 s after the peer's acknowledgment, frames traced (design, root cause); verify `cargo test -p rhizo-net a_connection_silent_for_long_still_gets_its_reply` fails before 2.1
- [x] 1.2 Realistic run: `rhizo up --arch x86_64 --accel tcg`, CPython `agent_chat.py` against a fake model answering after 45 s; verify the agent reports `Connection reset by peer` after about 40 s and the model server sees the retried request about 20 s after the first

## 2. Fix

- [x] 2.1 The card's timeout applies only while connecting or while sent bytes wait (D1), closed connections end by `LINGER` (D2); verify `cargo test -p rhizo-net wire_tests` (silent 45 s answer, 10-minute idle connection, unacknowledged bytes given up, reconnect after a reset and after the peer ends the connection)
- [x] 2.2 `Net::set_wire_timeout` and the in-kernel test `a_silent_connection_outlasts_the_timeout_and_reconnects_at_once` with the `quiet` cell (D3); verify `cargo xtask test --only ktest` on both architectures, and that the test fails without 2.1's change
- [x] 2.3 Realistic run again: two turns of a 45 s model 60 s apart on one connection; a model that closes after each answer, each next turn reconnecting in milliseconds; verify the model server's log and `rhizo chat`'s turn times

## 3. Gates

- [x] 3.1 No benchmark moves: verify `cargo xtask --arch x86_64 bench --vmm qemu-microvm` before and after for `net.tcp_rtt_us`, `cell.loopback_rtt_us`, `cell.tcp_resume_us` (and their instruction counts)
- [x] 3.2 Verify `cargo xtask check`, `cargo xtask --arch x86_64 test` and `cargo xtask test`
