# Design

## D1. One end for a cell's addresses, checked at every layer

`rhizo_cell::USER_END` (`0x7fff_ffff_f000`) is the end of what a cell may
use or name. It is checked where each kind of access starts:

- `memory::read`/`write` (every copy between a cell and the kernel) refuse
  a range past it before translating anything;
- `space::map_anonymous`, `unmap` and `protect` refuse such a range;
- `mprotect` answers `ENOMEM` and `arch_prctl(ARCH_SET_FS)` `EPERM` for it;
- the copy-on-write walk (`unshare_path`, `resolve_write`) never enters
  the upper half (a constant in `rhizo-mm`, which knows halves, not cells);
- the ELF loader and snapshots refuse pages past it.

The last page of the lower half is left out, as on Linux: code there could
end with a `syscall` whose return address is non-canonical.

Alternative: only check in the system calls. Rejected: the bug found
(`mprotect` reaching kernel entries through the x86_64 shared upper half)
was exactly one missing check at that layer; the lower layers now refuse
on their own, at the cost of a compare.

Alternative: give x86_64 cells no kernel half (separate tables, as on
AArch64). Rejected for now: every entry and exit would switch tables (the
cost KPTI pays); recorded as an open item with the side-channel limits.

## D1b. A shared frame is writable only through a copy

After a fork, read-only pages are shared without the copy-on-write mark
(no one can write them). `space::protect` checks the frame's reference
count before making a page writable and, if it is shared, asks
`PageTable::protect` for a copy-on-write entry instead: read-only until
the first write, which copies it.

Alternative: mark every shared leaf copy-on-write at fork. Rejected: the
mark means "writable once private", so a write to a read-only page would
then be resolved instead of faulting.

## D2. Range walks skip empty space by table entry

`PageTable::hole_end` returns where the empty region around an address
ends, at the level whose entry is empty. `space::unmap` steps over holes
with it, so a range costs one step per mapped page plus one per empty
entry (at most a few hundred for the whole half). `mprotect` brings pages
in (lazy resume) and stops at the first that cannot be: `ENOMEM`, as Linux
answers for an unmapped page in the range.

Alternative: cap the length of a range. Rejected: Linux accepts any length,
and libc unmaps large reservations; a cap would break programs, not just
attackers.

## D3. Names count against memory, and are bounded per cell

Names and inodes live in the kernel heap, which panics if it cannot grow.
Creating a file or directory first asks `Services::can_store` for 512
bytes (a high estimate of a name and its inode), the check file data
already passes, and each cell has at most 4096 files and directories
(`rhizo_fs::INODES_MAX`, `ENOSPC`, as tmpfs's inode limit). The bound also
limits what is built from names in one go: a path (so `getcwd`), a
directory listing. `getdents64` builds at most 64 KiB per call.

Alternative: account every heap byte to its cell. Better, but it needs an
allocator per cell; recorded as an open item (per-cell quotas).

## D4. Captured output is bounded

A host may capture a cell's output (tests, benchmarks). It is kept up to
1 MiB per cell and grown with `try_reserve`, so a cell printing forever
can neither fill the heap nor make it fail an allocation.

## D5. Refuse before zeroing

`mmap` and `brk` ask `can_store` for the whole length before mapping. Before,
a request larger than free memory zeroed frames until running out, then
gave them back: up to all of RAM zeroed per refused call, in a kernel that
is not preemptible.

## D6. Snapshots: registers the kernel would fault on are refused

`Snapshot::from_bytes` already checks pages and files; it now also checks
the program counter (both architectures) and, on x86_64, the thread
pointer and `MXCSR` reserved bits: an `IRET` to a non-canonical address,
a `WRMSR` of a non-canonical FS base and an `FXRSTOR` of reserved `MXCSR`
bits all fault in ring 0. New error `FormatError::BadState`.

## D7. Hostile tests that stay

- Host (`rhizo-abi/src/hostile.rs`): property tests drive `Abi::handle`
  with any number and arguments (kernel, non-canonical and last-page
  addresses, huge lengths, negative descriptors, small flags) on a cell
  whose table shares a kernel half holding tables, data and code, and
  check: no panic, results are values or errnos, kernel memory identical,
  messages bounded, every frame back. A second property checks `EFAULT`
  for every call that takes a pointer.
- In the kernel: hostile cells (`fuzz`, `hostile_ptrs`, `poke_kernel`,
  `hog`, `storm`, `scrub`, `namer`). The ones Linux answers the same way
  are in the differential check; `fuzz`, `hog` and `namer` are not (they
  would take the host's memory or make random calls on it).
