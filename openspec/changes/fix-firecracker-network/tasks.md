# Tasks

## 1. Evidence

- [x] 1.1 Capture the tap with tcpdump and Firecracker's net metrics during the in-kernel tests on Firecracker in CI (temporary workflow step, removed afterwards); verify the capture shows the host answering the guest's ARP and the metrics show `no_rx_avail_buffer` with no frame received

## 2. Fix

- [x] 2.1 `rhizo-virtio`: 64 descriptors per queue, compile-time checks of the queue layout and of Firecracker's minimum (D1); host test with a device that delivers as Firecracker does (D2), failing with 32 descriptors; verify `cargo test -p rhizo-virtio`
- [x] 2.2 `xtask` and CI: host network for `ktest-smp` on Firecracker, a one-CPU `ktest` step on Firecracker in CI, temporary diagnostics removed; verify `cargo xtask check`, `cargo xtask test` on both architectures

## 3. Verification

- [x] 3.1 CI green; the bench job measures `net.tcp_rtt_us/firecracker` and the Firecracker in-kernel steps run `a_cell_reaches_the_host`; verify the job log and `bench-results` annotations, and `openspec validate fix-firecracker-network --strict`
