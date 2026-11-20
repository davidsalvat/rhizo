# Tasks

## 1. Library crates

- [x] 1.1 `rhizo-hal`: `DeviceRegisters` and `NetLink` (D3); verify `cargo test -p rhizo-hal`
- [x] 1.2 Create `rhizo-virtio`: virtio-mmio v2 transport, split virtqueues, network device as a `NetLink`; host tests against a simulated device; add it to `xtask/src/deps.rs` and the AGENTS.md code map; verify `cargo test -p rhizo-virtio`
- [x] 1.3 `rhizo-net`: `smoltcp` wire (D2), DHCP and static addresses (D5); host tests against a `smoltcp` peer over an in-memory link; `deny.toml` allows 0BSD; verify `cargo test -p rhizo-net`
- [x] 1.4 `rhizo-core`: `ip=` and `echo=` options; verify host tests

## 2. Kernel, cells and tooling

- [x] 2.1 Kernel: virtio-mmio discovery and register mapping on both architectures, the card polled in the scheduler loop (D4); verify `cargo xtask test --only ktest` on both architectures shows the card and its DHCP address
- [x] 2.2 `xtask`: QEMU user network and host echo server, Firecracker tap when `sudo` allows; in-kernel test of a cell reaching the host; verify `cargo xtask test` on both architectures
- [x] 2.3 `net.tcp_rtt_us` in `bench-cell` and `xtask/src/bench.rs`, described in `bench/README.md`; verify `cargo xtask bench --runs 1` locally
- [x] 2.4 Frames: the card, the heap's single pages and cells' 4 KiB frames from 2 MiB blocks already in use (D7); verify `cargo test -p rhizo-mm` and the MicroPython in-kernel test with the card on AArch64

## 3. Documentation

- [x] 3.1 AGENTS.md (code map, VMM notes), `bench/README.md`; verify `cargo xtask ci` on both architectures and `openspec validate add-virtio-net --strict`
