# Tasks

## 1. The device and the stream

- [x] 1.1 `rhizo-virtio`: queues, feature negotiation and the virtio-mmio check shared by devices; PCI functions of any device type (`pci::regions`); descriptors chained; verify `cargo test -p rhizo-virtio` (the network card's tests unchanged and passing)
- [x] 1.2 `rhizo-virtio::vsock`: packet header, one stream (connection, credit both ways, resets) and the socket device (D6); verify `cargo test -p rhizo-virtio vsock`, with the property tests of hostile packets and credit overflow, and a simulated device

## 2. The kernel

- [x] 2.1 `kernel/src/vsock.rs` (`control` builds only): the first socket device found, polled from the control loop; the control channel follows the host's connection, hello on each (D2, D3); verify `cargo xtask --arch x86_64 test --only control` without a socket device (serial line)
- [ ] 2.2 Plain and benchmark kernels unchanged (D7); verify the release images' sizes on both architectures against `main`

## 3. The host

- [x] 3.1 `xtask/src/vmm.rs`: a socket device for control kernels on Firecracker, Cloud Hypervisor and QEMU with a usable `/dev/vhost-vsock` (D4); verify `cargo test -p xtask control_kernels`
- [ ] 3.2 `xtask/src/control.rs`: connect after the serial hello, wait for the vsock hello, send frames there (D3, D4); `rhizo up`'s relay uses it unchanged; verify `cargo test -p xtask the_vmm_s_socket` and the `control` step on `qemu-microvm`, `qemu-q35`, `firecracker` and `cloud-hypervisor` with KVM and on AArch64 `qemu-virt` (CI runners, design *Measurements*)
- [x] 3.3 Cloud Hypervisor's control kernel answers (D5); verify the `control` step on `cloud-hypervisor` and `cargo xtask --arch x86_64 bench --workload control` reporting its figures

## 4. Record

- [x] 4.1 `cargo xtask bench --workload control` before and after on one runner, in the design (*Measurements*)
- [ ] 4.2 `bench/baseline.json`: Cloud Hypervisor's control figures added in `ci`; existing `control.` values only lowered, from `bench` workflow runs of this branch
- [ ] 4.3 `bench/README.md` and `AGENTS.md` (code map): the channel
- [ ] 4.4 `openspec validate control-over-vsock --strict`, `cargo xtask check`, `cargo xtask ci` on both architectures
