# Tasks

## 1. Format

- [x] 1.1 Format version 9: waits on descriptors and kept call deadlines with their time left, `Snapshot::deadlines` (D5); verify `cargo test -p rhizo-snapshot waits_on_descriptors`
- [x] 1.2 Exported form with its CRC-32C, checked after the header, `export(&self)` (D1); verify `cargo test -p rhizo-snapshot` (`rejects_what_it_cannot_resume`, `the_checksum_is_crc32c`, `shared_pages_are_referred_to_*`)
- [x] 1.3 `cap_quota` and the hostile-snapshot property tests on the sealed form (D4); verify `cargo test -p rhizo-snapshot` (`quotas_from_outside_are_capped`, `hostile::*`, `records_that_refer_to_frames_are_refused`)

## 2. Kernel

- [x] 2.1 Deadlines across suspend and resume (D5); verify the in-kernel `a_suspended_wait_on_descriptors_keeps_its_time_left` on both architectures
- [x] 2.2 `Host::load_template`, `template_snapshot`, `drop_template` (D3); verify the in-kernel `a_template_travels_as_bytes` on both architectures

## 3. Control plane

- [x] 3.1 Protocol version 3: `freeze`, `save`, `load`, `load-data`, `data`, the `template` state, `BadSnapshot` (D2); verify `cargo test -p rhizo-control`
- [x] 3.2 The control kernel's templates, save and load, with the reasons of refusals (D2, D3, D4); verify the `control` test step on both architectures
- [x] 3.3 `rhizo freeze`, `save`, `load`; verify `cargo test -p rhizo-cli` and the `control` test step: the agent's template and suspended cell across `rhizo down` and `rhizo up`, refused files

## 4. Benchmarks, documentation, CI

- [x] 4.1 Snapshot file metrics on QEMU (D6); verify the control-plane benchmark (`control::measure_on`) on `qemu-microvm` with CPython built reports the five metrics (figures in the design)
- [x] 4.2 `docs/audit-scope.md` (B3, area 6) and `docs/security.md` (snapshots from files); verify by reading
- [ ] 4.3 Everything CI runs; verify `cargo xtask check`, `cargo xtask test` on both architectures with `RHIZO_CPYTHON_SRC`, and CI
