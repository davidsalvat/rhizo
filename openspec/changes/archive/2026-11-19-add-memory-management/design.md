# Design

## Context

See `proposal.md` for motivation and `specs/` for required behavior. Current
state (H0): the PVH trampoline identity-maps the first 1 GiB with 2 MiB pages
and the kernel is linked at 1 MiB. There is no IDT, no allocator, and the
hardware boundary consists of `Console`, `Power` and `CycleCounter`.

## Goals / Non-Goals

**Goals:**

- Every allocator and page-table algorithm lives in `rhizo-core` and is
  tested on the host (unit and property tests) against in-memory doubles.
- Deterministic behavior: the same memory map always yields the same
  allocation sequence.
- A layout that leaves the whole lower half free for cells (H2).

**Non-Goals:**

- Returning heap pages to the frame allocator (size-class pages are kept).
- Freeing page-table frames when a range becomes empty.
- Per-CPU data, interrupts from devices, user-mode segments (H2).

## Decisions

### D1. Address-space layout

| Range | Contents |
|---|---|
| `0x0000_0000_0000_0000 – 0x0000_7fff_ffff_ffff` | Empty (future cells) |
| `0xffff_8000_0000_0000 + p` | Direct map of physical RAM (2 MiB pages, RW, NX) |
| `0xffff_ffff_8000_0000 + p` | Kernel image (4 KiB pages, W^X), linked here |

The kernel is linked at `0xffff_ffff_8000_0000 + 1 MiB` with physical load
addresses (`AT()`) at 1 MiB, so the hypervisor still loads it at 1 MiB. The
top-2-GiB placement matches the `kernel` code model used by the
`x86_64-unknown-none` target.

Alternatives: keep the kernel identity-mapped low (rejected: collides with
cell address spaces in H2); place the kernel right after the direct map
(rejected: outside the `kernel` code model).

### D2. Two-stage page tables

- **Stage 1 (assembly trampoline):** static tables map physical 0–4 GiB three
  times: identity (needed while switching to long mode), the direct map, and
  the first 1 GiB at the kernel base. The trampoline enables `EFER.NXE` and
  `CR0.WP`, switches to the kernel stack in the higher half and calls
  `kernel_main(start_info_phys, entry_tsc)`.
- **Stage 2 (Rust):** builds fresh tables from allocated frames: direct map of
  RAM (up to the highest RAM address below 4 GiB) and the kernel image with
  per-section permissions. The guard page below the kernel stack is left out.
  Loading the new `CR3` removes the identity mapping.

The trampoline code and its data live in a small `.boot` section linked at the
physical address; it is unreachable after stage 2.

Alternative: build final tables in assembly. Rejected: untestable and hard to
change.

### D3. Hardware boundary for memory

`rhizo-hal` gains:

- `PhysicalMemory`: read and write a `u64` at a physical address, zero a
  frame. Kernel implementation: the direct map. Host double: a sparse map.
- `Tlb`: invalidate the translation for one page. Kernel implementation:
  `invlpg`. Host double: records flushed pages.

Page-table code in `rhizo-core` only uses these traits, so the exact same code
runs against fake memory in host tests. Loading `CR3` and reading `CR2` stay in
`kernel/src/arch/`.

### D4. Frame allocator: bitmap with next-fit

One bit per 4 KiB frame (1 = in use), covering physical memory below 4 GiB
(a 128 KiB static bitmap). Everything starts as in use; RAM regions from the
memory map are then marked free and the first 1 MiB, the kernel image and the
boot information are marked in use again. Allocation scans from a next-fit
hint; contiguous runs are found by scanning for N clear bits.

Alternatives: free-list stack (O(1) but cannot provide contiguous runs),
buddy allocator (more complexity than needed now). Frame reference counts for
copy-on-write are deferred to H5.

### D5. Memory map source

PVH `hvm_start_info` version 1 provides `memmap_paddr` and `memmap_entries`;
each entry is `{ addr: u64, size: u64, type: u32, reserved: u32 }` and type 1
means RAM. Parsing reads through `PhysicalMemory`, caps the number of entries
at 128 and rejects versions below 1.

### D6. Kernel heap: size classes plus whole pages

`rhizo-core` provides a heap built on a `PageSource` trait:

- Requests up to 2048 bytes (after rounding the size up to the alignment and
  to a power of two, minimum 16) use one of eight size classes (16 to 2048).
  Each class carves 4 KiB pages into equal blocks and keeps freed blocks in an
  intrusive free list. Blocks of a power-of-two size inside a page-aligned page
  are naturally aligned to their size.
- Larger requests take contiguous pages directly and give them back on free.
- Alignments above 4096 fail (null), which the global allocator turns into the
  standard out-of-memory panic that includes the requested size.

The kernel's `#[global_allocator]` wraps the heap in a single-CPU cell that
panics on reentrance; its `PageSource` takes contiguous frames and returns
their direct-map addresses.

Alternatives: `linked_list_allocator` or `talc` crates (permissive licenses,
but we want full control over determinism and, later, per-cell accounting).

### D7. CPU exceptions

- New GDT (kernel code `0x08`, kernel data `0x10`, TSS `0x18`) and a TSS whose
  IST1 points to a dedicated 16 KiB double-fault stack.
- IDT entries 0–31 point to small assembly stubs that push a uniform frame
  (vector, error code or 0, CPU frame) and call one Rust handler. Vector 8
  (double fault) uses IST1.
- Report formatting (exception names, page-fault error decoding) lives in
  `rhizo-core` and is host-tested; the kernel adds `CR2` for page faults and
  exits with failure.

### D8. Self-checks

Three new kernel features add a deliberately faulting test to the suite, each
driven by `cargo xtask test`: `ktest-page-fault` (write to `0x0`),
`ktest-stack-overflow` (unbounded recursion) and `ktest-invalid-opcode`
(`ud2`). Each must exit with failure and print the expected report.

### New external dependencies

`proptest` (MIT OR Apache-2.0), host dev-dependency only.

## Risks / Trade-offs

- [Higher-half switch bugs triple-fault with no output] → the IDT is installed
  before stage 2, and QEMU's `-d int,cpu_reset` is documented for debugging.
- [RAM above 4 GiB is ignored] → accepted for the PoC; documented in the spec.
- [Size-class pages are never returned] → acceptable for a kernel heap at this
  stage; revisit when per-cell accounting arrives.
- [Direct map covers holes below the top of RAM with cacheable mappings] → no
  code touches them; MMIO mappings get their own attributes when devices arrive.
