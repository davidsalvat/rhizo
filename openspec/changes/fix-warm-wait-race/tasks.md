# Tasks

## 1. Reproduce

- [x] 1.1 Simulated 2 ms host stall in `run_slice` after a block while the output ends in `warm\n` (not committed); verify `RHIZO_CPYTHON_SRC=… RHIZO_NODE=1 cargo xtask --arch x86_64 test --only ktest` fails (`copies_draw_different_random`: `two copies drew the same`)

## 2. Fix

- [x] 2.1 `rhizo_core::line::Line` (D2, D3); verify `cargo test -p rhizo-core line` (the piece of an unseen pause, partial and other lines, a property test with arbitrary splits)
- [x] 2.2 `Live::hold` and `Host::run_to_pause` (D1, D2); the template tests and benchmarks of CPython, MicroPython, Node.js, numpy, the dynamically linked `hello` and `libc_warm`, and the `copies` and `netstate` tests, wait through it; verify every in-kernel test passes with a 60 ms stall after `warm\n`, `ready\n` or `start\n`, and without it, on x86_64 (AArch64 builds CPython only natively, so CI runs it there)

## 3. Gates

- [x] 3.1 Verify `openspec validate fix-warm-wait-race --strict`, `cargo xtask check`, and `cargo xtask ci` on both architectures
- [x] 3.2 No benchmark regression, templates frozen at the same point: verify `cargo xtask bench --icount-only` on both architectures
