# Tasks

## 1. Review

- [x] 1.1 Threat model per new area and the hostile snapshot as a second attacker in `docs/security.md`; verify by reading against the code
- [x] 1.2 Findings S14–S21 with severity, status, how each was confirmed, and what stays open; verify `docs/security.md` lists each with its test

## 2. Fuzzers

- [x] 2.1 `decoded_files_cannot_hurt_the_kernel`, `random_bytes_are_refused_or_work` (`rhizo-abi`); verify `cargo test -p rhizo-abi hostile`
- [x] 2.2 `tampered_snapshots_are_refused_or_contained`, `random_bytes_are_refused_or_contained` (`rhizo-snapshot`, D2); verify `cargo test -p rhizo-snapshot hostile` and 60000 cases with another seed
- [x] 2.3 `tampered_programs_start_safely_or_not_at_all` (`rhizo-abi`); verify `cargo test -p rhizo-abi exec`
- [x] 2.4 `any_configuration_space_is_set_up_or_refused` (`rhizo-virtio`); verify `cargo test -p rhizo-virtio`
- [x] 2.5 The weekly job runs them with a random seed and 4096 cases; verify the workflow file

## 3. Fixes

- [x] 3.1 S16: saturating sums of `iovec` lengths; `buffer_lengths_that_overflow_their_sum_are_harmless` panicked before; verify `cargo test -p rhizo-abi buffer_lengths`
- [x] 3.2 S18: `ElfError::BadEntry` from `start_at` (D3); `entry_points_outside_the_cell_are_refused`; verify `cargo test -p rhizo-abi exec`
- [x] 3.3 S17: BARs with a power-of-two size that does not wrap; verify `cargo test -p rhizo-virtio`
- [x] 3.4 S19: `dup2` on 32-bit descriptors; the saved failing case in `proptest-regressions`; verify `cargo test -p rhizo-abi hostile`

## 4. Hostile cells

- [x] 4.1 `racer` (`io`, `futex`, `exit`, `protect`) and `threads_racing_on_several_cpus_cannot_hurt_the_kernel` (D4); verify `cargo xtask --arch x86_64 test --only ktest-smp`
- [ ] 4.2 Everything on both architectures; verify `cargo xtask test`, `cargo xtask test --only ktest-smp` and `cargo xtask --arch x86_64 test`
