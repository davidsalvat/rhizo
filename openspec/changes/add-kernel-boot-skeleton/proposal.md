# Proposal

## Why

Rhizo has no code yet. Before any agent-cell primitive can be built, we need a
kernel that boots, can report what it is doing, and can be tested
automatically, both on the host and inside a virtual machine. Getting the test
harness and the hardware boundary right now is cheap; retrofitting them later
is very expensive. This is roadmap milestone **H0**.

## What Changes

- A Rust kernel for x86_64 that boots directly under QEMU (no separate
  bootloader image), prints a boot banner to the serial console and halts
  cleanly.
- A narrow hardware boundary: kernel core code reaches the console and power
  control only through interfaces, with the x86_64/QEMU implementation kept
  in a separate module.
- An in-kernel test harness: a test build of the kernel runs its test cases
  inside QEMU and reports pass/fail to the host through the VM exit code.
- Host-side unit tests for hardware-independent code, runnable with the
  standard Rust test command.
- A single developer command to build, run and test the kernel, so
  contributors need only the Rust toolchain and QEMU.
- Continuous integration running formatting, lints, host tests and in-QEMU
  tests on every pull request.
- Kernel panics are reported on the serial console and end the VM with a
  failure status instead of hanging.

### Out of scope

Memory management (H1), user mode and cells (H2+), interrupts and timers beyond
what booting requires, multicore, any device other than the serial port and the
QEMU exit device, UEFI/BIOS bootloaders, real hardware, ARM.

## Capabilities

### New Capabilities

- `kernel-boot`: how the kernel starts on its supported platform, what it
  reports on the console and how it stops (normally or on panic).
- `kernel-testing`: how kernel behavior is verified, on the host and inside a
  virtual machine, and how results are reported to developers and CI.

### Modified Capabilities

None.

## Impact

- New Cargo workspace with the kernel crate, a hardware-independent core
  library and a developer task runner.
- New external tooling requirement for contributors: QEMU (x86_64 system
  emulator).
- New GitHub Actions workflow.
- `AGENTS.md` and `README.md` gain the build and test commands.
