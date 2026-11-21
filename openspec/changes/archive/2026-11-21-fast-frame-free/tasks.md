# Tasks

## 1. Profile

- [x] 1.1 Profile frame allocation, its lock and zeroing (cost accounting under `-icount`, lock waits in the 4-vCPU server workload) and record the figures and the decision (D1); verify the figures are in the pull request

## 2. Freeing

- [x] 2.1 `free_contiguous` checks and frees a word at a time, with a one-bit path for one frame (D2); verify `cargo test -p rhizo-mm frames`
- [x] 2.2 In-kernel tests unchanged; verify `cargo xtask test` and `cargo xtask --arch x86_64 test`

## 3. Measuring

- [x] 3.1 Instruction counts before and after on both architectures (`cargo xtask --arch <a> bench --icount-only`), no figure worse; verify the comparison in the pull request
