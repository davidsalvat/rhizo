# Tasks

## 1. Fuzzer

- [x] 1.1 `frames_from_the_wire_cannot_hurt_the_kernel` and `dhcp_replies_cannot_hurt_the_kernel` in `crates/rhizo-net/src/hostile.rs` (D1, D2, D4), with the attacker's limits of D3; verify `cargo test -p rhizo-net hostile::`
- [x] 1.2 `seeds_reach_what_they_aim_at` and `a_valid_lease_configures_the_card`: the seeds reach their targets; verify `cargo test -p rhizo-net hostile::`
- [x] 1.3 A long run: three random seeds for 45 minutes each; verify no failure and no `proptest-regressions` file left

## 2. Fixes

- [x] 2.1 S22: at most `POLL_FRAMES` frames per `poll_wire`, which then returns `Some(0)` (D5); `a_flood_from_the_wire_lets_the_kernel_go` failed before; verify `cargo test -p rhizo-net flood`
- [x] 2.2 S23: recorded only, fixed on `main` by `fix-tcp-long-wait` (D6); verify the fuzzer's 20 s and 30 s waits pass after the merge

## 3. Documentation and checks

- [x] 3.1 The review of B5 and findings S22 and S23 in `docs/security.md`, with how to run the fuzzer; B5 and Q11 marked covered in `docs/audit-scope.md`; verify by reading
- [x] 3.2 The weekly job runs the fuzzer with a random seed; verify the workflow file
- [x] 3.3 `openspec validate fuzz-wire-packets --strict`, `cargo xtask check` and `cargo xtask --arch x86_64 test`
