# Tasks

## 1. Flags

- [x] 1.1 `flags` cell and in-kernel test, with the debug assertion in `enter_user`, failing before the fix (D2); verify `cargo xtask --arch x86_64 test --only ktest` fails, then passes with `push 2; popfq` in `rhizo_user_trap`

## 2. Guards

- [x] 2.1 SMEP and SMAP on every x86_64 CPU that has them (D1); verify `user_guards_are_on` and `cargo xtask --arch x86_64 test --only ktest-smp`
- [x] 2.2 PAN and cleared SPAN on every AArch64 CPU that has it (D1); verify `pan_is_on` and `cargo xtask test --only ktest-smp`
- [x] 2.3 Self-checks `ktest-user-read` and `ktest-user-exec` (D3); verify `cargo xtask test --only ktest-user-read` and `--only ktest-user-exec` on both architectures

## 3. Documentation and CI

- [x] 3.1 `docs/security.md`: O5 closed, finding S13, the direct map's place; verify by reading
- [x] 3.2 Everything CI runs; verify `cargo xtask ci` on both architectures and CI green
