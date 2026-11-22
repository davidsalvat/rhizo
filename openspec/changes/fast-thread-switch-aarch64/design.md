# Design

## D1. The kernel's `memcpy` family on AArch64

**What ran before.** The kernel is built without `build-std`, so
`memcpy`, `memmove`, `memset` and `memcmp` come from the prebuilt
`compiler_builtins` of `aarch64-unknown-none-softfloat` (disassembled
from the release kernel of the parent commit): `memcpy` aligns the
destination with a byte loop, then copies 8 bytes per 4 instructions when
the source is aligned alike, or assembles each word from 8 byte loads
(about 24 instructions per 8 bytes) when it is not; `memcmp` is a byte
loop; `memset` stores 8 bytes per loop. The compiler calls `memcpy` for
every move of a value too large to inline: a thread record (about 860
bytes, 528 of them FP/SIMD state), a `Live`, socket buffers, snapshot
sections.

**What replaces it.** Strong global symbols in
`kernel/src/arch/aarch64/mem.rs` (`global_asm!`), which the linker takes
over `compiler_builtins`' weak ones:

- `memcpy`: 64 bytes per loop, four `LDP` then four `STP` of general
  registers with post-indexed addressing (10 instructions per 64 bytes);
  the remaining 0–63 bytes, like any copy shorter than 64, by size class
  with one access at each end that may overlap: 1–3 (bytes at 0, n/2 and
  n−1), 4–7 (two words), 8–16 (two doublewords), 17–32 (two pairs),
  33–63 (four pairs). Every class loads all its bytes before it stores
  any.
- `memmove`: forward through `memcpy`'s code when `dst − src ≥ n`
  (unsigned: the destination starts before the source or past its end),
  else backward from the ends, 64 bytes per loop, and the head as a short
  copy. A loop step loads its 64 bytes before storing them and moves away
  from what it stored, so overlapping ranges are copied as through a
  buffer; the short classes need nothing more since they load first.
- `memset`: the byte multiplied into all eight lanes, then the same size
  classes, or 64 bytes per loop and the last 64 at the end.
- `memcmp`: 16 bytes per loop (two `LDP`, `CMP`, `CCMP`), then 8, then
  single bytes; the first differing doubleword is byte-reversed (`REV`)
  so an unsigned compare orders it as the bytes are ordered.

Unaligned accesses are allowed: Rust code runs only with the MMU on, on
normal memory, with `SCTLR_EL1.A` clear (the target is built
`strict-align`, so the compiler itself never emits them, but the CPU
handles them). Only x0–x17 are used (caller-saved, no stack). No SIMD
register is used: the kernel stays soft-float, and a cell's vector
registers stay live in the CPU across kernel entries (change
`lazy-fp-switch`).

Alternatives:

- Copy 128 bytes per loop: about 25 instructions fewer for a thread
  record, with eight more registers and a longer tail; 64 keeps one
  structure for copy, move and fill.
- Align the destination first: costs instructions under `-icount` on
  every call, for a gain on hardware not shown here (no AArch64 KVM host
  available); records and buffers are 8-byte aligned already.
- `DC ZVA` in `memset` for long zero fills: whole pages already go
  through `zero_page`; shorter fills gain little.
- FEAT_MOPS (`CPYP`/`SETP`): not on the CPUs Firecracker and Cloud
  Hypervisor run on (design D3 of `fast-mem-primitives`).
- Keep `compiler_builtins` and avoid the copies only (D2 alone): every
  other copy of the kernel stays slow (see D4).

x86_64 keeps `compiler_builtins`' `rep movsq`/`rep stosq` routines: no
x86_64 code changes.

**Host tests.** The routines are AArch64 assembly; the host is x86_64 and
cannot run them, so there is no host reference model. The in-kernel test
`memory_routines_match_a_byte_model` (shared suite, both architectures)
calls the four symbols directly, so a constant length cannot let the
compiler inline the call, and checks every byte against a pattern
function, never with a routine under test: every length 0–160 at every
pair of offsets modulo 16 (`memcpy`, `memcmp` equal and with one byte
changed), `memset` at 16 offsets, `memmove` at every distance 0–72 in
both directions, whole pages, and 300 random lengths and offsets up to
4,096. Sixteen bytes on each side of the destination are checked
unchanged. A broken `memmove` (always forward) fails it at once (`memmove
of 65 from 100 to 101`). It takes about 4 s of the in-kernel suite under
TCG.

## D2. A thread switch trades two records

`Threads::next` made the next runnable thread current with
`others.remove(i)` (a copy out and a `memmove` of the threads after it),
`mem::replace` of the current thread (two copies) and `others.push` (one
copy): four record copies and a `memmove` per switch, even with two
threads.

It now copies the next thread out, the current one into its slot, and the
next one into the current place (three copies through `memcpy`), then
rotates `others[i..]` left by one so the order is the same as before (the
skipped threads, then the ones after `i`, then the previous current
thread). With two threads the rotation is empty. This is `rhizo-cell`'s
first `unsafe` block (three lines with a `// SAFETY:` comment):
`core::mem::swap` is safe but, on this strict-alignment target, swaps a
value this size byte by byte, and measured worse than before (15,087
instructions for `cell.loopback_rtt_insns` instead of 8,447).

The current thread stays inline in `Threads`: `current()` and
`current_mut()` are unchanged, so the system-call path pays nothing on
either architecture.

## D3. Rejected: records that never move

A table indexed by the current thread (the #139 experiment) or a
pointer to a boxed current thread removes the copies entirely:

- the #139 table (branch on every current-thread access) brought
  loopback to about 7,730 but failed `cell.syscall_insns` (349–351
  against 331) and `cell.pipe_rtt_insns` (2,062) on x86_64;
- an index without a branch still adds a dependent load and a multiply to
  every access, and the order in which threads take turns and are woken
  would follow the ring rather than today's queue (the `threads` spec
  and tests describe the queue);
- a boxed current thread allocates for every cell, so spawn and fork
  (gated at +2 %, x86_64 `cell.fork_insns` already at 7,030 against
  6,970) would pay an allocation and a copy.

With D1 and D2, AArch64 loopback is 7,776, within 1 % of the table's
figure, and nothing else changes.

## D4. Measurements

`cargo xtask --arch aarch64 bench --icount-only --env ci-arm --check` and
`cargo xtask --arch x86_64 bench --icount-only --env ci --check`, on the
parent commit (`before`), with D1 alone (AArch64, `memcpy only`) and with
D1 and D2 (`after`). Instruction counts under `-icount` are deterministic
(one run each). The CPython workloads were not built locally; the x86_64
gate fails only on their missing figures, every measured figure passes.

| Metric | AArch64 before | memcpy only | after | change | x86_64 before | after | change |
|---|---:|---:|---:|---:|---:|---:|---:|
| `cell.cow_fault_insns` | 1,040 | 1,040 | 1,040 | +0.0 % | 1,045 | 1,045 | +0.0 % |
| `cell.file_write_read_insns` | 9,034,069 | 7,646,111 | 7,646,107 | −15.4 % | 3,947,621 | 3,947,619 | −0.0 % |
| `cell.fork_insns` | 7,712 | 6,064 | 6,064 | −21.4 % | 7,030 | 7,030 | +0.0 % |
| `cell.fork_touch_insns` | 8,533,968 | 8,531,584 | 8,531,584 | −0.0 % | 6,844,987 | 6,844,985 | −0.0 % |
| `cell.fork_with_files_insns` | 7,680 | 6,039 | 6,039 | −21.4 % | 6,998 | 6,998 | +0.0 % |
| `cell.loopback_rtt_insns` | 12,000 | 8,447 | 7,776 | −35.2 % | 8,674 | 8,077 | −6.9 % |
| `cell.pipe_rtt_insns` | 1,898 | 1,770 | 1,770 | −6.7 % | 2,030 | 2,030 | +0.0 % |
| `cell.resume_eager_insns` | 1,371,653 | 1,365,403 | 1,365,403 | −0.5 % | 1,098,680 | 1,098,680 | +0.0 % |
| `cell.resume_insns` | 11,746 | 5,456 | 5,456 | −53.6 % | 4,311 | 4,311 | +0.0 % |
| `cell.resume_touch_insns` | 1,383,899 | 1,376,915 | 1,376,911 | −0.5 % | 1,120,863 | 1,120,933 | +0.0 % |
| `cell.run_insns` | 94,604 | 92,100 | 92,098 | −2.6 % | 88,585 | 88,584 | −0.0 % |
| `cell.snapshot_export_insns` | 6,386,025 | 3,475,577 | 3,475,577 | −45.6 % | 2,946,138 | 2,946,138 | +0.0 % |
| `cell.snapshot_import_insns` | 2,124,230 | 2,123,030 | 2,123,030 | −0.1 % | 1,857,478 | 1,857,478 | +0.0 % |
| `cell.spawn_from_template_insns` | 7,664 | 6,032 | 6,032 | −21.3 % | 6,992 | 6,992 | +0.0 % |
| `cell.spawn_insns` | 21,764 | 19,952 | 19,952 | −8.3 % | 28,156 | 28,156 | +0.0 % |
| `cell.suspend_insns` | 4,654,204 | 2,273,510 | 2,273,510 | −51.2 % | 2,290,904 | 2,290,904 | +0.0 % |
| `cell.switch_domains_insns` | 1,290 | 1,289 | 1,287 | −0.2 % | 1,374 | 1,373 | −0.1 % |
| `cell.switch_fp_insns` | 2,067 | 2,067 | 2,065 | −0.1 % | 1,785 | 1,784 | −0.1 % |
| `cell.switch_insns` | 826 | 826 | 824 | −0.2 % | 873 | 872 | −0.1 % |
| `cell.switch_untagged_insns` | 829 | 829 | 827 | −0.2 % | 872 | 871 | −0.1 % |
| `cell.syscall_insns` | 278 | 278 | 278 | +0.0 % | 333 | 333 | +0.0 % |
| `cell.tcp_resume_insns` | 79,497 | 60,743 | 59,736 | −24.9 % | 63,011 | 62,110 | −1.4 % |
| `workload.cold_insns` | 36,518,575 | 36,515,324 | 36,515,316 | −0.0 % | 35,246,402 | 35,246,398 | −0.0 % |
| `workload.warm_insns` | 73,743 | 71,362 | 71,360 | −3.2 % | 76,892 | 76,891 | −0.0 % |

Reading:

- Loopback: D1 removes about 3,550 instructions (the record copies at
  8 bytes per 4 instructions, and the payload copies), D2 another 670
  (one record copy and the `memmove` call per switch, two switches per
  round trip). x86_64 gains D2 only.
- Suspend and snapshot export halve: their byte buffers were copied at
  unaligned offsets, the 24-instruction path. Resume (lazy) halves for the
  same reason; fork, spawn from a template and file reads and writes
  drop 15–21 %.
- Whole-page paths (`fork_touch`, eager resume, copy-on-write faults) do
  not change: they already used `copy_pages` and `zero_page`.
- `cell.syscall_insns` and `cell.pipe_rtt_insns` on x86_64 do not move;
  no gated figure rises on either architecture. The baselines are left
  as they are (a later archive round lowers them; AArch64
  `cell.loopback_rtt_insns` is not in the `ci-arm` baseline yet).
