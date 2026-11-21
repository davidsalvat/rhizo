# Design

## Context

Every cargo feature set of the `kernel` crate is a separate compilation
of it (its dependencies are shared: no kernel feature reaches them).
Before this change, after an edit to kernel code (`touch
kernel/src/main.rs`; this host: 4 CPUs, no KVM). "Compiled" counts the
`kernel` crate's compilations and lints; the times are the wall time of
the cargo calls that build or lint the kernel, first with each kernel
compiled in full (`CARGO_INCREMENTAL=0`, the cost of an edit that
reaches most of the crate), then with incremental reuse (a small edit):

| Command | Compiled, per arch | Build time, full (aarch64 / x86_64) | Build time, incremental | Lint time, incremental |
|---|---|---|---|---|
| `cargo xtask check` | 2 linted (one per arch) | — | — | 2.0 s |
| `cargo xtask test` | 11 built | 58.5 s / 36.6 s | 7.6 s / 9.0 s | — |
| `cargo xtask ci` | 11 built, 15 linted | 31.3 s / — | 7.5 s / 9.3 s | 43.4 s / 29.3 s |

The 11 are the plain kernel (boot check), `ktest` (in-kernel tests on 1
and 4 CPUs), `control` (the control plane through `rhizo`) and one
kernel per harness self-check. `ci` also lints 15 feature sets: the
plain kernel, `ktest`, `bench-cell`, `bench-cell,profile`, `control`,
`ktest,packages`, `bench-cell,packages` and the eight self-checks.
`RHIZO_NODE` and `RHIZO_CPYTHON_SRC` add one kernel each (`ktest,node`,
`ktest,packages`).

## Goals

- At most about 3 kernel compilations per architecture for a full `test`
  or `ci`, plus the large-embed kernels only where their variables are set.
- Every test and self-check kept, with the same expected results and
  outputs, the same step names and reproduce commands.
- Test code stays behind one build-time test feature; kernels without it
  are unchanged byte for byte, so no benchmark (time or `-icount`) moves.

## Decisions

### D1. Self-checks become `selfcheck=NAME` on the test kernel

The eight `ktest-*` features only chose the test list: each built `ktest`
plus one function and a list of two cases (`console_output_works`, then
the case that must fail). `kernel/src/tests.rs` now compiles every
self-check function in every `ktest` build and lists them in
`SELF_CHECKS`, by name; `tests::chosen` returns the named one's list, or
the suite (`ALL`) when the command line names none. `kernel_main` passes
the command-line buffer it already has. The arch helpers that only
self-checks use (`cpu::execute_invalid_instruction`, `user::load_tables`)
are compiled under `ktest` instead of their own features.

An unknown name panics (`unknown self-check `NAME``): a misspelled option
fails the step instead of quietly running the suite, which the step would
also fail but with a less useful report. Checked on aarch64 by renaming
one option in `xtask`: the step reported the panic and failed.

The option is parsed by `rhizo_core::options::self_check`, a function of
its own placed after the existing ones, not a `BootOptions` field: kernels
without tests never call it, so the linker drops it, and no line above
it moves (panic locations are part of the image). That is what keeps the
other kernels byte-identical (D5).

Alternatives:

- *A `BootOptions::self_check` field.* Every kernel would parse a word it
  never uses: the boot path and image of the benchmarked kernels change.
- *Run the self-checks inside the suite.* They end the VM by design (a
  fault, a hang): each still needs its own boot.

### D2. `ktest` and `ktest-smp`: already one kernel

They differed only in the VM's CPUs (`-smp`, `vcpu_count`, `--cpus`) and
the card's transport, both chosen outside the image; the kernel was
already built once for both. The self-checks now join them.

### D3. What stays compile-time, and why

| Feature | Stays because |
|---|---|
| `ktest` | The one test-capable switch. Test and self-check code (functions that fault, hang or read user pages on purpose, test-only helpers) must not exist in kernels users boot; it is chosen once per build, never per test. |
| `cells` | Implied by `ktest`, `bench-cell`, `control`; the plain kernel without cells is the boot-time benchmark (`boot-performance`), 40 KiB (aarch64) and 64 KiB (x86_64) of loadable bytes against 5 to 6 MiB with the cell host and the embedded cells. |
| `control` | It adds a UART receiver and the request loop, and `cargo xtask test`'s `control` step checks the kernel `rhizo up` boots, built with the same features and no test code (and `check_kernel_reused` checks that build is not compiled again). A control mode in the test kernel would test a different kernel. One debug compile of about 3 s. |
| `bench-cell` | A release kernel; the test kernel is a debug one, so they never share a build. Folding it into the plain kernel would embed the cells into the boot benchmark's image; folding it into `control` would put benchmark code into the kernel users run and change what the `-icount` gates measure. Its workloads are already chosen at boot (`bench=laptop`, `bench=server`, `cells=`, `secs=`). |
| `node`, `packages` | They embed Node.js (over 100 MiB) and the packages (65 MiB). Every kernel carrying them would load, copy and decode them (each such VM needs 1 GiB): built only where `RHIZO_NODE` or `RHIZO_CPYTHON_SRC` is set, and only in their own kernels. |
| `profile`, `sampling` | Cost-accounting hooks sit on the hottest paths: every system call (`cost::syscall`), every part a cell enters (`cost::scope`, opened and closed), every `Global` lock acquisition, every timer tick (`cost::timer_sampling`). Without the feature they compile to nothing. A runtime switch needs at least a load and a branch per hook point (2 instructions on x86_64): a system call crosses at least four (its call, the cell part, entry and exit), so at least 8 more instructions on `cell.syscall_insns`, 332 on `qemu-microvm-icount` with a +2 % gate (6.6 instructions). The gate would move: they stay compile-time. |

### D4. `xtask`

- `SELF_CHECKS` rows carry the step name (`ktest-page-fault`) and the
  boot option (`page-fault`) instead of a feature.
- `cmd_test` builds the test kernel once if `ktest`, `ktest-smp` or any
  self-check is wanted, and boots it for each, the self-checks with
  `RunOptions::selfcheck`.
- `vmm.rs` adds `selfcheck=NAME` to the command line next to `shard=`
  for QEMU, Firecracker and Cloud Hypervisor.
- `cmd_ci` lints 7 feature sets instead of 15: the self-check code is
  linted with `ktest`.

### D5. No benchmarked kernel changes

The loadable bytes (`objcopy -O binary` of the ELF) of every kernel built
without `ktest` were compared before and after the change, built from the
same path (the build script's output path is embedded): plain,
`bench-cell`, `control`, `bench-cell,profile`, `bench-cell,sampling`,
release and debug, on both architectures. All 20 are identical, so no
benchmark metric can move; the `-icount` gates were run as well.

## Results

After the change, with the same edit and host:

| Command | Compiled, per arch | Build time, full (aarch64 / x86_64) | Build time, incremental | Lint time, incremental |
|---|---|---|---|---|
| `cargo xtask check` | 2 linted (one per arch) | — | — | 2.0 s |
| `cargo xtask test` | 3 built | 7.4 s / 6.7 s | 1.6 s / 2.4 s | — |
| `cargo xtask ci` | 3 built, 7 linted | 5.3 s / — | 1.7 s / 1.9 s | 28.9 s / 14.0 s |

The kernel compile time of `test` drops by a factor of 5 to 8. The whole
commands gain less on this host: under emulation (no KVM) the in-kernel
test VMs take 75 to 80 s and set the wall time (`test` took 116 to 256 s
before and 119 to 194 s after, depending on the run). `check` builds no
kernel and is unchanged. The aarch64 full `ci` figure is lower than its
`test` one because `test` compiled the dependencies first.

## Risks

- A self-check now runs on a kernel that also holds the suite: a broken
  test elsewhere can stop a self-check kernel from compiling, as it
  already stopped `ktest`. Both fail at build time, with the error.
- The command-line buffer is 256 bytes; `selfcheck=` runs carry no
  network options, so they stay well within it.
