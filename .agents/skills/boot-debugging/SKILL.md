---
name: boot-debugging
description: Debug a Rhizo kernel that does not boot, prints nothing, resets, or faults early.
---

# Debugging boot

1. Reproduce with the console visible: `cargo xtask run` (QEMU microvm) or
   `cargo xtask run --machine q35`.
2. No output at all: the console choice comes from the PVH command line
   (`console=e9|serial`). Try `--machine q35` and check the `-append` value
   with `RHIZO_PRINT_VMM_COMMAND=1`.
3. Silent reset: run QEMU by hand with `-d int,cpu_reset -D qemu.log` (print
   the exact command with `RHIZO_PRINT_VMM_COMMAND=1`) and look at the last
   exception before the reset.
4. A reported exception: map `rip` to source with
   `addr2line -f -e target/x86_64-unknown-none/debug/rhizo-kernel <rip>`.
5. Step through: `cargo xtask run --gdb`, then
   `gdb target/x86_64-unknown-none/debug/rhizo-kernel -ex 'target remote :1234'`.
6. Early boot code lives in `kernel/src/arch/`; everything after
   `kernel_main` can use the boot console.
