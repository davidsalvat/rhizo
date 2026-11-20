# Tasks

## 1. Memory pressure

- [x] 1.1 Reserve in `KernelFrames` (D1); `Services::can_store` and `ENOSPC` on file writes, with a host test (D2)
- [x] 1.2 `suspend` and eager `resume` refuse what does not fit (D2)
- [x] 1.3 In-kernel tests: flood with `ballast` and with `filer`; kernel survives, memory returns

## 2. Density

- [x] 2.1 `idle` cell (endless, so not in the differential run) and the 1000-cell in-kernel test
- [x] 2.2 `cell.idle_bytes` and `cell.suspended_bytes` (D3)

## 3. Documentation

- [ ] 3.1 README roadmap (H9), `bench/README.md`, `cargo xtask ci` on both architectures
