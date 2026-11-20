# Tasks

## 1. Generator

- [x] 1.1 `rhizo_hal::Entropy` and `testing::FakeEntropy` (D3); verify `cargo test -p rhizo-hal`
- [x] 1.2 ChaCha20 block function and fast-key-erasure generator in `rhizo_core::random` (D1); verify `cargo test -p rhizo-core random` (RFC 8439 vectors)

## 2. Kernel

- [x] 2.1 `arch::random::HardwareEntropy` on both architectures (D3); verify the in-kernel `hardware_entropy_works_where_present` on both
- [x] 2.2 Kernel generator seeded at start, per-cell generators split on first use, `AT_RANDOM` and the network seed from it (D2); verify the in-kernel `no_two_cells_draw_the_same_random_bytes` on both architectures

## 3. Documentation and CI

- [x] 3.1 `docs/security.md` (O4 closed, `getrandom` row) and `AGENTS.md` (arch interface); verify by reading
- [ ] 3.2 Everything CI runs; verify `cargo xtask ci` on both architectures and CI green
