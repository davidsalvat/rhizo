# Tasks

## 1. The interrupt

- [x] 1.1 The transports' interrupts from the device tree and the command line (D1); verify `cargo test -p rhizo-core fdt`
- [x] 1.2 `route_device` on both architectures, device vector 35 on x86_64 (D2); verify `cargo xtask test` on both architectures
- [x] 1.3 `VirtioNet::pending`, `Net::take_wire_work`; verify `cargo test -p rhizo-virtio` and `cargo test -p rhizo-net calls_that_give_smoltcp_work_say_so`
- [x] 1.4 Polling only when due (D3); verify `--only ktest` on both architectures (`the_cards_interrupt_wakes_the_idle_cpu`)

## 2. Measuring

- [x] 2.1 `laptop.turn_wakeups` (D4); verify `cargo test -p xtask laptop_line_and_marks_give_the_metrics` and a local `cargo xtask bench --workload laptop`
- [ ] 2.2 CI figures for every VMM in the pull request; the metric in the `ci` baseline from two agreeing runs; verify the CI benchmark job
