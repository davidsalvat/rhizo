# Design

## D1. Guards always on, no user-access window

The kernel reaches a cell's memory only through `rhizo_cell::memory`
(`read`, `write`, `read_str`...), which translate the cell address in the
cell's own table and copy through the direct map
(`0xffff_8000_0000_0000 + phys`, supervisor pages). So the kernel never
needs to touch a user page by its user address, and SMAP/PAN can stay on
all the time:

- x86_64: `user::init_tags` (boot CPU) and `user::init_cpu` (the others)
  set CR4.SMEP and CR4.SMAP for the features `CPUID.7.0:EBX[7]` and
  `[20]` list.
- AArch64: `user::init_cpu` sets `PSTATE.PAN` and clears `SCTLR_EL1.SPAN`
  when `ID_AA64MMFR1_EL1.PAN` is non-zero. With SPAN clear, every
  exception taken to EL1 sets PAN, so the kernel is guarded again after
  each return from a cell. Execution is already refused: cell pages are
  mapped `PXN`.

The direct map is what the guards cannot see: a kernel bug that computed
a wrong physical address could still reach a cell's frame. That is
covered by the software checks (the cell's own table, the user and write
bits on the effective permissions, `USER_END`), documented in
`docs/security.md`.

Alternative: toggle the guards around each copy (`stac`/`clac`, PAN
off/on), as Linux does for `copy_from_user`. Rejected: Rhizo never copies
through the user address, so a window would only add cost (and an
instruction that faults on CPUs without SMAP).

Alternative: copy through the user address with the guards toggled, and
drop the software walk. Rejected: the walk is what checks that the
address belongs to *this* cell's table while the CPU may hold another
one's, and it is needed anyway for copy-on-write and lazy pages.

## D2. A cell's flags stay in the cell (finding S13)

An x86_64 interrupt gate clears IF, TF, NT and RF, but not DF or AC.
`rhizo_user_trap` now runs `push 2; popfq` right after `SWAPGS`, before
any other instruction, so Rust code never sees the direction flag a cell
set (the ABI requires it clear: `rep movs` would run backwards) and SMAP
is never lifted by a cell's AC. `SYSCALL` already masks both (`SFMASK`).
Confirmed before the fix by the `flags` cell and a debug assertion after
every return from a cell (DF and AC were both set: `0x40400`).

AArch64 has no equivalent: the PSTATE fields EL0 can write (NZCV, and
SSBS/DIT/TCO where present) do not change how EL1 code runs memory
accesses, and PAN is set by the exception itself.

## D3. Tests that would fail without the guards

- Self-checks `ktest-user-read` and `ktest-user-exec`: the kernel maps a
  user page (holding a `ret`) in a fresh lower half, loads it, and reads
  it or calls it. The run must end with the CPU's fault report (x86_64:
  protection violation, kernel mode; AArch64: permission fault). Without
  the guards the read and the call succeed and the run ends with a panic
  instead, which the harness rejects.
- `user_guards_are_on` / `pan_is_on`: the bits are set (after cells ran).
- `flags_a_cell_sets_stay_in_the_cell`: the `flags` cell next to `abi`
  cells; the debug assertion in `enter_user` checks every return.
