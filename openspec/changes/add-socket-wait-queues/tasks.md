# Tasks

## 1. Wait queues

- [x] 1.1 Keyed waiting set and ordered deadlines in `rhizo-sched`: `wait(key, …)`, `wake`, no wake-all (D2); verify `cargo test -p rhizo-sched` (only the named waiters wake, deadlines never early, property tests)
- [x] 1.2 Wait queues per socket in `rhizo-net`: sockets noted by the calls, `end_call`, `take_woken`, `forget`, each change waking the socket of the table (D1); verify `cargo test -p rhizo-net` (`a_change_wakes_only_the_waiters_of_its_socket`, `accept_waits_for_a_connection_and_a_writer_for_room`, `closing_wakes_the_waiters_of_the_socket_and_its_peer`, `frames_from_the_wire_wake_the_waiters_of_its_sockets`)
- [x] 1.3 `Tables::changes` in `rhizo-fs` and the ABI waking the cell's own waiting threads (D3); verify `cargo test -p rhizo-fs what_a_waiting_thread` and `cargo test -p rhizo-abi a_pipe_write_wakes`
- [x] 1.4 The kernel: joining the queues at the end of a call, `wake_woken` and `nudge` under the host's lock, new cells making their calls again, keys forgotten when a cell ends (D2); verify `cargo xtask test --only ktest` and `--only ktest-smp` on both architectures (`only_the_cells_waiting_on_a_socket_wake`, `forked_servers_share_their_listener`, `a_waiting_cell_uses_no_cpu`)

## 2. Measuring

- [x] 2.1 `IoWoken` and `IoWasted` counts, `density.wasted_wakeups_pct` (D4); verify `cargo test -p xtask workloads` and `cargo xtask bench --workload server --cells 1000 --vmm qemu-microvm --runs 1` locally
- [ ] 2.2 Before and after figures of the server workload from CI in the pull request; verify the CI benchmark job
- [ ] 2.3 Gate `density.sched_p99_us` and `density.wake_p99_us` from two CI runs that agree, `density.wasted_wakeups_pct` ungated with its reason (D4); verify `cargo xtask bench --from <run> --env ci --check` on both runs

## 3. Documentation

- [ ] 3.1 `bench/README.md`: the new metric, the p99 latencies gated, the breakdown line; verify by reading it next to a CI run's output
