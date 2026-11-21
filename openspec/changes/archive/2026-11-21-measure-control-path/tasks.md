# Tasks

## 1. Measure

- [x] 1.1 `control.spawn_to_first_output_us` and `control.fork_to_first_output_us` in `xtask/src/control.rs`, with their breakdown (D1); verify `cargo xtask --arch x86_64 bench --workload control --vmm qemu-microvm` reports them
- [x] 1.2 The serial line's cost per byte each way, the kernel's time for a `list`, an idle round trip (D2); verify the same run reports `breakdown.control.serial_in_ns_per_byte`, `serial_out_ns_per_byte`, `roundtrip_kernel_us` and `idle_roundtrip_us`
- [x] 1.3 `--workload control` in `xtask/src/bench.rs` and the usage text; verify `cargo test -p xtask options` (its parsing test)
- [x] 1.4 A `bench` workflow run of this branch with KVM (run 44929147780); verify the breakdown explains each `control.` figure from its bytes (design *Measurements*)

## 2. Cheap win

- [x] 2.1 x86_64 16550: control frames sent with one line-status read per 16 bytes with the FIFOs on, the console's text as before (D3); verify `cargo xtask --arch x86_64 test --only control` (byte-exact transcripts) and the default release image's size unchanged
- [x] 2.2 Two `bench` workflow runs with it (44930748675, 44931429957); verify out against in per byte drops, and both pass the `ci` gate

## 3. Record

- [x] 3.1 Design: measured breakdown, bottleneck, follow-up `control-over-vsock` (D4)
- [x] 3.2 `bench/baseline.json`: the two new gated metrics in `ci`, the mean of those two runs; no existing value changed
- [x] 3.3 `bench/README.md`: the new metrics and the breakdown
- [x] 3.4 `openspec validate measure-control-path --strict`, `cargo xtask check`, `cargo xtask ci` on both architectures
