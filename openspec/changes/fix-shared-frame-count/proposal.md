# Proposal

## Why

The second adversarial review (D.2 in `docs/security.md`, finding S14)
found that one cell can panic the kernel. Every mapping of a file page
takes one more reference to the page cache's frame (design D1 of
`add-dynamic-linking`), and the kernel counts a frame's references in 16
bits, with a panic past 65535 ("more than 65535 cells share a frame").
The first review rated that limit low (O7) because only the host could
reach it, by forking one cell 65536 times. File mappings changed that: a
cell that writes one byte to a file of its own and maps the file's first
page 70000 times stops the kernel and every other cell. Reproduced in the
kernel under QEMU (x86_64) before the fix.

## What Changes

- `Frames::try_share`: adds a reference only if the count can hold one
  more, and says whether it did. The default keeps today's `share`; the
  kernel's frames refuse at 65535 extra references, under the lock of the
  counts.
- The page cache uses it: when the kept frame of a page cannot take one
  more reference, the caller gets a copy of the page in a frame of its own
  (not kept), as if the page were not cached. Nothing a cell does panics,
  and no count wraps.
- A hostile test cell, `sharer`, and an in-kernel test that runs it next
  to another cell and checks that every frame comes back.

### Out of scope

The panic stays on host-only paths (O7 in `docs/security.md`): forking one
cell, or spawning one kept image, 65536 times while every copy is alive.
A cell cannot reach them.

## Capabilities

### Modified Capabilities

- `isolation`: hostile calls get answers, not crashes, also when a cell
  makes the kernel share one frame without end.

## Impact

- `crates/rhizo-mm` (`cow::Frames`), `crates/rhizo-cell` (test frames),
  `crates/rhizo-abi` (`PageCache`), `kernel/src/cell.rs`.
- A cache hit does the same work under the same lock (a checked add
  instead of a panicking one); no benchmark should move.
