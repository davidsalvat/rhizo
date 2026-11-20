# Design

## D1. File pages kept in frames, by the identity of their contents

A cell's files are blocks of the kernel heap (`rhizo-fs`, `Arc` per 4 KiB
block, shared copy-on-write between forks and between every cell given
the same files). A mapped page must be a frame. `rhizo_abi::PageCache`
keeps one frame per file page, keyed by the address of the block that
holds it and the number of file bytes in it (`rhizo_fs::Page::key`). The
cache holds a clone of the block (`Page`): while it does, the block cannot
change in place (a write to the file, in any cell, copies it first) and its
address cannot be reused, so equal keys mean equal bytes. A page with no
data (a hole, or past the end) gets a zeroed frame of the mapping's own,
not kept.

Mappings take a reference to the kept frame (`Frames::share`); a frame
another mapping still uses is mapped copy-on-write when the mapping is
writable (`space::map_frames`). So every cell of `ls`, and every library
`ld.so` maps in it, shares `libc.so.6`'s 2 MiB; data pages are copied on
first write. The kernel holds the cache in a global (`PAGES`), takes its
frames like kept executables' (never from the reserve), makes a page
written into a frame visible to instruction fetch once (AArch64), keeps at
most 16384 pages (64 MiB; past that it starts over), and drops it with the
kept executables before any cell is refused memory (`drop_images`).

Alternatives: copying each mapped page into a private frame (what the
static loader does): every spawn of `python3` would copy its 6.5 MiB and
`libc`, and forks would no longer share them. Mapping the heap blocks
themselves: they are heap allocations, not frames with reference counts,
and freeing a file would have to find its mappings. Keying by path or
inode: a file written after it was cached would map stale bytes; keying by
a hash of the contents: hashing every page at each mapping costs more than
the lookup it saves.

## D2. One loader: executables and interpreters mapped from file pages

`elf::Image::parse_file` validates an executable or shared object
(`ET_EXEC` or `ET_DYN`, with `PT_INTERP` and `PT_DYNAMIC`) from its first
page and the file's size; segments must map file pages (same offset within
a page). `Image::map_file` maps each segment as Linux does: pages of the
segment's file part are file pages (shared, copy-on-write where writable),
the page where the file part ends inside `.bss` is a private copy with the
rest zeroed, and pages of `.bss` alone are zeroed. `rhizo_cell::map_program`
maps an executable and its interpreter; `rhizo_abi::exec` finds both in the
cell's files and maps them through the page cache; `Host::spawn_file`
starts the cell. Static executables in the files take the same path.

Interaction with the image cache (`add-image-cache`): executables embedded
in the kernel (`Host::spawn`, `&'static` images, keyed by address) are
still loaded by copy and kept as spaces. Executables in the files are not
put in the image cache: their pages are already shared through the page
cache, by content, so a second spawn builds tables and takes references
without copying a byte, and a file written between two spawns can never
start from a stale space. A kept space per file would save only the table
building (tens of pages for `ld.so` and the executable).

## D3. Where things go

A position-independent executable loads at `PIE_BASE` (4 GiB): below the
mapping area, so its heap (`brk`, from the end of the executable up to
`MMAP_BASE`) has room, and aligned for any segment alignment. The
interpreter loads at the first mapping address, `MMAP_BASE`, aligned as its
segments ask, and the mapping cursor continues above it (Linux maps it in
the mapping area too). Both are fixed: no randomness in the kernel (rule 2;
`AT_RANDOM` stays the only random input). The auxiliary vector gains
`AT_BASE` only when there is an interpreter, so static cells' stacks are
what they were. `AT_PHDR` and `AT_ENTRY` describe the executable, which
`ld.so` relocates from them.

## D4. Inaccessible pages hold nothing

`ld.so` makes the gap between a library's segments `PROT_NONE`; glibc does
the same for guard pages and arena tails. Rhizo has no other record of a
mapping than its page tables, and an inaccessible page cannot be a
page-table entry: a kernel-only entry in the lower half would be global on
AArch64 (shared by every address space) and is not a page the snapshot
format accepts; an invalid entry that keeps its frame would have to be
known to every walker (unmap, fork, snapshot, teardown). So `mprotect` to
`PROT_NONE` (and `MAP_FIXED` with `PROT_NONE`) unmaps the pages and frees
their memory; in the mapping area the addresses stay reserved, as
`PROT_NONE` mappings already are, and a later `mprotect` backs them with
zeroed pages. Linux keeps the contents; no program here makes pages
inaccessible and expects their bytes back (glibc discards arena tails
itself).

## D5. Test inputs: the runner's own distribution

`xtask/src/dynamic.rs` builds a tree and encodes it (`dynamic-files`,
embedded like `cpython-files`):

- `usr/bin/hello`: `cells/c/dynamic/hello.c` built with the cells' C
  compiler without `-static`; it prints whether the auxiliary vector gave
  it an interpreter base, and with `wait` pauses after `ready` (for
  suspend, fork and templates).
- Natively only (as the differential check, and as CPython): the host's
  `/usr/bin/ls`, `/usr/bin/sort` and the interpreter `/usr/bin/python3`
  names, with `cells/python/dynamic_demo.py` as `app/demo.py` and the
  standard-library modules it uses compiled by that interpreter
  (`pack_stdlib.py`, now also copying extension modules such as `_json`,
  which `python3` opens with `dlopen`, and always `os`, the landmark by
  which CPython finds its library), and `data/` for `ls` and `sort`.
- For every ELF file in the tree, its interpreter at the path `PT_INTERP`
  names and its `DT_NEEDED` libraries (and theirs) under `lib/<triple>`, a
  directory `ld.so` searches by default (no `/etc/ld.so.cache`: its absence
  makes `ld.so` search those directories), found with `readelf` in the
  host's system directories, or in the cross toolchain's sysroot
  (`/usr/<triple>/lib`) when cross-building. Symbolic links are resolved:
  each file is at the path programs look for.

CI's runners are native on both architectures (`ubuntu-24.04`,
`ubuntu-24.04-arm`), so both run everything; a cross build runs the
dynamic `hello` only and the tests say what was not packed. The binaries
are the runner's (glibc LGPL-2.1-or-later, coreutils GPL-3.0-or-later,
libselinux public domain, PCRE2 BSD-3-Clause, OpenSSL Apache-2.0, zlib,
expat MIT, CPython PSF-2.0): test inputs run unmodified, never copied into
the repository (rule 1 is about code in the repository).

## D6. No shared file mappings

`MAP_SHARED` of a file fails with `ENODEV` (Linux's answer for a file that
cannot be mapped so). Nothing packed needs it; supporting it means frames
written by several cells and written back to their files, which cells do
not share (each has its own copy-on-write file system).

## D7. Snapshots, forks and templates

A mapped file page is memory of the cell like any other: a snapshot
captures its bytes (copy-on-write pages as writable, as after a fork), so
a snapshot is self-contained and resumes without the files it was mapped
from; restored pages are the resumed cell's own. A fork or a template
shares every page copy-on-write, kept file pages included, so cells forked
from a `python3` template share its interpreter and libraries. The page
cache is the kernel's, never part of a cell's state.

## D8. Metrics

`workload.dynamic_run_us`: spawning the dynamic `hello` from the files and
running it to its exit (`ld.so` maps `libc`, relocates, runs `main`),
the robust figure of the cell benchmarks (the lowest of five rounds'
medians) over 25 runs after a first that fills the page cache, against
`cell.run_us` (a static `hello`). `workload.dynamic_template_us`: the same
program forked from a template frozen at `ready`, after `ld.so` did its
work: what an agent platform pays once relocations are absorbed. Both are
new; no baseline exists for them until a CI run records one.
