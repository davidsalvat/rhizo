# Tasks

## 1. Root cause

- [x] 1.1 Read `Stream::take`, `pump` and `Server::read`: a reset and a new request taken in one `pump` leave `connected()` true, so no hello and the old decoder (design, root cause); verify with the packets of 2.1 against the old `Stream` (connected throughout, and the old reader gets the new connection's bytes)

## 2. Fix

- [x] 2.1 `Stream::connection` and `Stream::read(connection)` (D1); verify `cargo test -p rhizo-virtio vsock` (`a_reconnect_between_two_polls_is_a_new_connection`)
- [x] 2.2 The control kernel keeps the connection's number, greets and renews its decoder on any change, and reads no byte of a new connection before (D2); verify `cargo xtask test --only control` on both architectures

## 3. Gates

- [x] 3.1 Verify `cargo xtask check`
- [x] 3.2 No benchmark regression: verify `cargo xtask --arch x86_64 bench --icount-only --env ci --check` (every measured metric within its threshold; the CPython workloads need `RHIZO_CPYTHON_SRC` and are measured by CI)
