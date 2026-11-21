# Tasks

## 1. The gate

- [x] 1.1 `rhizo-virtio::vsock::Gate` over `Stream` (`Stream::peer`, `Stream::reset` public): admission by the first 32 bytes, constant-time comparison, refusal count, the 2 s bound, replacement of an unadmitted connection (D2); verify `cargo test -p rhizo-virtio -- vsock::` (wrong, missing, slow and split secrets, a reconnect after a drop, a stranger first, property tests `only_the_secret_admits` and `hostile_packets_are_never_admitted`)

## 2. The kernel

- [x] 2.1 `kernel/src/vsock.rs`: the secret from the kernel's generator, its console line before the hello, no vsock without a hardware random source, refusals reported (D1); verify `cargo clippy -p rhizo-kernel --features control` on both architectures and `cargo xtask test --only control` on both (serial line)

## 3. The host

- [x] 3.1 `xtask/src/control.rs`: the secret line taken out of the console text, the secret sent first on every vsock connection, the serial line when none was given (D3); verify `cargo test -p xtask control::` (`the_secret_line_never_reaches_the_console_text`, `a_vsock_connection_presents_the_secret_first`)
- [x] 3.2 `rhizo up`: state directory `0700`, control socket `0600` (D3); verify `cargo test -p rhizo-cli the_state_directory_is_private`
- [x] 3.3 The `control` step: strangers without the secret are reset with nothing sent back and cannot power the VM off; `console.log` holds no secret; the state directory is `0700`; verify the `control` step in PR CI over vsock (task 3.4)
- [x] 3.4 The vsock path runs in PR CI (D6): `vhost_vsock_usable` loads the module before looking for the device; the step prints its channel and, on a CI runner with a socket device (Firecracker, Cloud Hypervisor, QEMU with KVM), fails on a fallback to the serial line; the KVM job runs the step on Firecracker and Cloud Hypervisor; verify the PR's `x86_64 tests (host and QEMU)` and `x86_64 tests on Firecracker and Cloud Hypervisor (KVM)` jobs green, with `frames over vsock, strangers checked` in their logs

## 4. Record

- [x] 4.1 `docs/security.md`: O8 closed as finding S28; `.github/workflows/weekly.yml` runs `vsock::` with a random seed
- [x] 4.2 `openspec validate authenticate-control-channel --strict`, `cargo xtask check`
- [x] 4.3 No benchmark regression: `cargo xtask bench --icount-only --check` on both architectures (`--env ci` and `--env ci-arm`); the CPython workloads need `RHIZO_CPYTHON_SRC` and are measured by CI
