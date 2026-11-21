# Tasks

## 1. Measure

- [x] 1.1 Breakdown of the Node.js warm forks (`profile`, local) under `-icount` and TCG, with the template frozen at `warm` and in its pause; verify the 50 ms of `idle` per fork when frozen in the pause, and nothing else of that size (design, table)
- [x] 1.2 Check the benchmark: the busy wait after `freeze` and what `spawn_from_template` and `Host::run` wait for; verify without the busy wait the first fork waits the rest of the pause (49 ms) and the others none (design)

## 2. Fix

- [x] 2.1 `Live::expire_timers` times out waits on descriptors, used by `spawn_from_template`, `Host::wake` and a kept host wake (D1); `sleeper epoll` and the in-kernel test `a_wait_on_descriptors_keeps_no_time` (D3); verify `cargo xtask --arch x86_64 test --only ktest` passes and hangs on the test without D1's change
- [x] 2.2 `Host::waits` and the Node.js template frozen in its pause (D2); verify `RHIZO_NODE=1 cargo xtask --arch x86_64 bench --workload node --accel icount --vmm qemu-microvm --no-icount --no-reference --runs 1`: `workload.node_warm_us` 79.5 M before 2.1, 29.6 M after, `workload.node_cold_us` unchanged (250 M)

## 3. Gates

- [x] 3.1 Verify `openspec validate fast-node-template --strict` and `cargo xtask check`
- [x] 3.2 Verify `RHIZO_NODE=1 cargo xtask --arch x86_64 test` and `cargo xtask test`
- [ ] 3.3 CI with KVM: `workload.node_warm_us` below `workload.node_cold_us` on every VMM, no other metric worse
