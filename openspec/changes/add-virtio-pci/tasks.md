# Tasks

## 1. Library crates

- [x] 1.1 `rhizo-hal`: 8- and 16-bit `DeviceRegisters` accesses (D1); verify `cargo test -p rhizo-hal`
- [x] 1.2 `rhizo-virtio`: `pci` module (recognition, BAR sizing and assignment, capabilities, D2 and D3) and the PCI transport behind the shared driver (D1); host tests against a fake configuration space and the simulated device; verify `cargo test -p rhizo-virtio`
- [x] 1.3 `rhizo-core`: ECAM from the ACPI MCFG and the device tree's PCI memory window; verify `cargo test -p rhizo-core`

## 2. Kernel and tooling

- [x] 2.1 Kernel: `platform::{pci_ecam, pci_window, map_device}` on both architectures, the PCI scan in `wire.rs` (D3, D4); verify `cargo xtask --arch x86_64 test --only ktest --vmm qemu-q35` and `cargo xtask --arch aarch64 test --only ktest-smp` report a card `at virtio-pci` and pass
- [x] 2.2 `xtask`: `q35` and `virt` PCI cards, Cloud Hypervisor tap, `q35` in-kernel tests in `ci` (D5); verify `cargo xtask test` on both architectures and CI's Cloud Hypervisor in-kernel run
- [x] 2.3 `net.tcp_rtt_us` on `q35` and Cloud Hypervisor; verify CI's benchmark job reports both

## 3. Documentation

- [ ] 3.1 AGENTS.md (VMM notes), `bench/README.md`; verify `cargo xtask ci` on both architectures and `openspec validate add-virtio-pci --strict`
