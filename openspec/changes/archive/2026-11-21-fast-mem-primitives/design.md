# Design

## D1. Where pages are zeroed and copied, and what ran there

Every zero and copy of a page goes through `rhizo_hal::PhysicalMemory`,
which the kernel implements with `memory::DirectMap`:

| Path | Call |
|---|---|
| Fresh frames: anonymous memory, page tables, ELF bss, write-or-execute pages backed on first touch (`node-jit`), page cache | `zero_frame` (`rhizo-cell` `space`, `elf`; `rhizo-mm` `paging`, `cow`; `rhizo-abi` `pages`) |
| Copy-on-write of a 4 KiB page or a 2 MiB block (fault path, clones with own tables, fork's child writing) | `copy` (`rhizo-mm` `cow`) |
| Suspend (pages into the snapshot), export (shared frames into the file's bytes) | `read_bytes` (`rhizo-snapshot`) |
| Eager resume, lazy page-in | `write_bytes`, `copy` (`rhizo-snapshot` `restore_page`) |
| File blocks of `rhizo-fs` | `Arc::make_mut` of `[u8; 4096]`: the compiler's `memcpy`, not through the trait (out of scope) |

`DirectMap` called `core::ptr::write_bytes` and `copy_nonoverlapping`,
which in a `no_std` kernel are `memset` and `memcpy` from
`compiler_builtins` (disassembled from the release kernels of the parent
commit):

- AArch64: an 8-byte `ldr`/`str` loop, 4 instructions per 8 bytes when
  source and destination are aligned alike (2,048 per page), and a loop
  of byte loads assembled into words, 22 instructions per 8 bytes,
  otherwise; `memset` stores 8 bytes per 3 instructions (1,536 per page).
- x86_64: `rep movsb` (head, up to 7 bytes), `rep movsq`, `rep movsb`
  (tail); the same with `stos` for `memset`.

Under `-icount` on the parent commit, AArch64 spent 26.2 of the 26.7
million instructions of the `fork_touch` breakdown in `copy`, and 2,480
instructions per copy-on-write fault; x86_64, 6.56 of 7.02 million and
1,049.

## D2. x86_64: `rep stosq` and `rep movsq`

`zero_page` is one `rep stosq` of 512 quadwords and `copy_pages` one
`rep movsq` of `512 * n` quadwords, with no head or tail: callers pass
whole pages. Both are fast-string operations on every x86_64 CPU and use
general registers only.

Alternatives:

- `rep movsb`/`rep stosb` with ERMS or FSRM (checked with `CPUID`): QEMU
  counts every iteration of a `rep` string instruction as one retired
  instruction, so a page costs 4,096 instead of 512 under `-icount`
  (measured in D7: the gated figures rise), while Intel's guidance gives
  ERMS no advantage over `movsq` for 4 KiB copies with aligned
  destinations. No KVM host was available to time both; without a timing
  that shows a gain, the variant that keeps the instruction gate wins.
- `memcpy` as before: the head and tail `rep movsb` with a count of zero
  still start up (two microcoded instructions per page on hardware, two
  counted instructions under `-icount`).

## D3. AArch64: `DC ZVA` and pairs of x registers

`zero_page` reads `DCZID_EL0` (one `mrs`, no trap): unless `DZP` is set it
zeroes the page with `DC ZVA` of the block size `4 << BS` bytes (64 on
QEMU's `max` and on Neoverse cores: 64 instructions per page plus the
loop), else with 16-byte `STP` of `xzr`, 128 bytes per loop.
`copy_pages` moves 128 bytes per loop with eight `LDP`/`STP` pairs of
general registers. Unaligned addresses are allowed (`SCTLR_EL1.A` is
clear, and the direct map and kernel heap are normal memory, which `DC
ZVA` needs).

Alternatives:

- `LDP`/`STP` of `q` registers: the kernel is built soft-float and does
  not save a cell's vector registers on entry; using them would clobber
  the cell's state (and needs the FP unit on at EL1).
- FEAT_MOPS (`CPYP`/`SETP`): newer than the CPUs Firecracker and Cloud
  Hypervisor run on (Graviton 2 and 3, Ampere Altra), so it would only add
  a third path behind a check.
- Non-temporal `STNP`: pages zeroed or copied are touched right after (the
  faulting cell writes the page, the resumed cell runs), so keeping them in
  the cache is the point.

## D4. Through `PhysicalMemory`, platform-free libraries

The library crates keep calling the trait; only the kernel's
implementation knows the instructions. `DirectMap::zero_frame` calls
`zero_page`; `copy` calls `copy_pages` for a whole number of pages and
`memcpy` otherwise. The trait gains `read_pages` and `write_pages`
(defaults: `read_bytes` and `write_bytes`), which `rhizo-snapshot` uses
for its page data, and which `DirectMap` implements with `copy_pages`.

Alternative: test the length inside `read_bytes` and `write_bytes`. Small
copies of user memory are their common case, and the test alone cost
x86_64 eleven instructions per pipe round trip (`cell.pipe_rtt_insns`
2,035 to 2,046, over the gate), where `memcpy` was already `rep movsq`.

## D5. CRC-32C passed in

`rhizo-snapshot` is a library and holds no assembly: `export` and
`from_bytes` take a `Crc32c` (`fn(&[u8]) -> u32`), and the table version
(`rhizo_snapshot::crc32c`) is public, the reference and the fallback. The
kernel passes `arch::mem::crc32c`, which checks once (`CPUID` leaf 1
`ECX.SSE4_2`; `ID_AA64ISAR0_EL1.CRC32`, both of which may trap to the
hypervisor, so the answer is kept in an atomic) and then runs `crc32`
(64-bit) or `CRC32CX`, four words per loop, then words, then bytes for
the tail. The loads are in the assembly: `aarch64-unknown-none-softfloat`
is `strict-align`, so the compiler builds an unaligned word from eight
byte loads (that version cost 25 million instructions to export 8 MiB,
against 6.4 million), while the CPU loads it at once from normal memory;
on x86_64 `crc32` reads its operand from memory. Written this way the
loop is also fast in the unoptimized test kernels.
Same polynomial, same reflection, same initial value and final inversion:
the bytes of a file do not change, which the in-kernel test checks
against the table for every length 0 to 63, around one and two pages, at
every offset modulo 8, and a host property test checks the table against
the bit-by-bit definition.

Alternatives: a global hook set at boot (hidden state in a library); three
interleaved streams combined by carry-less multiplication (faster on
hardware, where one `crc32` chain is latency-bound at about 8 bytes per
3 cycles, but needs `PCLMULQDQ`/`PMULL`, SIMD registers the kernel does not
use).

## D6. Metrics

`cell.snapshot_export_us` and `cell.snapshot_import_us` time, in the cell
benchmark's eager rounds, the 8 MiB `ballast` snapshot exported with its
checksum and those bytes checked and decoded: what saving and loading a
snapshot file cost inside the kernel, without the network
(`snapshot.save_us` and `load_us` need CPython and QEMU and are not
gated). Both are counted under `-icount` like every cell figure. The page
primitives show in `cell.cow_fault_*`, `cell.fork_touch_*`,
`cell.suspend_*`, `cell.resume_eager_*`, `cell.resume_touch_*`,
`cell.spawn_*` and the `copy` parts of the breakdowns.

## D7. Results

Retired instructions under QEMU `-icount` (`cargo xtask bench
--icount-only`), parent commit against this change:

| Figure | AArch64 before | after | x86_64 before | after |
|---|---|---|---|---|
| `cell.cow_fault_insns` | 2,480 | 1,040 (-58%) | 1,049 | 1,045 |
| `cell.fork_touch_insns` | 26.7 M | 8.66 M (-68%) | 6.98 M | 6.98 M |
| `cell.suspend_insns` | 7.59 M | 4.68 M (-38%) | 2.32 M | 2.32 M |
| `cell.resume_eager_insns` | 4.31 M | 1.39 M (-68%) | 1.12 M | 1.12 M |
| `cell.spawn_insns` | 51,536 | 21,671 (-58%) | 28,328 | 27,821 (-2%) |
| `workload.warm_insns` | 92,390 | 73,790 (-20%) | 77,099 | 76,863 |
| `workload.cold_insns` | 39.8 M | 36.6 M (-8%) | 35.4 M | 35.3 M |
| `breakdown.fork_touch.copy_insns` | 26.2 M | 8.19 M (-69%) | 6.56 M | 6.56 M |

The new figures, table checksum against the CPU's (both with the page
primitives): `cell.snapshot_export_insns` 57.2 M to 6.39 M on AArch64
(-89%) and 32.9 M to 2.95 M on x86_64 (-91%); `cell.snapshot_import_insns`
52.9 M to 2.12 M (-96%) and 31.8 M to 1.86 M (-94%).

The ERMS variant of D2 (`rep movsb`/`rep stosb`) on x86_64:
`cell.cow_fault_insns` 4,712, `cell.fork_touch_insns` 52.9 M,
`cell.resume_eager_insns` 8.56 M, and fourteen gated figures over their
threshold.

Under x86_64 `-icount` the page primitives change little: `memcpy` was
already `rep movsq`; what they save there (two `rep movsb` start-ups and
the alignment arithmetic per page) shows on hardware, which no KVM host
here could time.
