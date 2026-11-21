# Tasks

## 1. Reproduce

- [x] 1.1 Root cause from the CI log of job 136959808283 and the code (design, root cause): only the last turn's boundary can be missed, and only when its 1 ms pause expires inside the slice; verify by reading `run_slice`, `Threads::next` and the card's polling
- [x] 1.2 Simulated 2 ms host stall in `run_slice` (not committed), applied to the agent's pause after a turn; verify `RHIZO_CPYTHON_SRC=… cargo xtask --arch x86_64 test --only ktest` fails with `suspended after every turn`, left 2, right 3

## 2. Fix

- [x] 2.1 `rhizo_core::agent::Watch`, a line reader for the agent's output (D1, D3); verify `cargo test -p rhizo-core agent` (the piece CI saw, incomplete and other lines, a property test with arbitrary splits)
- [x] 2.2 `agent()` reads every new byte through `Watch`, suspends after each turn's line even with the decision point in the same piece, freezes once the decision point is read and the agent sleeps, and reports turns read together as an error (D1, D2); verify the in-kernel test passes with the simulated stall of 1.2, and without it on x86_64 (AArch64 builds CPython only natively, so CI runs it there)

## 3. Gates

- [x] 3.1 Verify `cargo xtask check`, and `cargo xtask ci` on both architectures
- [x] 3.2 No benchmark regression: verify `cargo xtask bench --icount-only`
