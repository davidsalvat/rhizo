# Proposal

## Why

The laptop reference workload (`add-reference-workloads`) opens 50 agents
from one template and suspends 45 of them. A live agent costs 27.6 KB,
because it shares the template's pages copy-on-write, but its snapshot
costs 1.28 MB: suspending copies every page it maps, the ones it still
shares with the template included. The 45 snapshots are 95 % of the
memory the workload takes (60.4 MB of 61.1 MB on QEMU). Suspending is
what a laptop does with the agents its user is not talking to, so a
suspended agent should cost about what a live one does, not 46 times as
much. Copying also makes suspending slow (1.4 ms per agent under QEMU
TCG, most of it the copy).

## What Changes

- A snapshot taken in the kernel refers to the frames its cell shares
  with another cell (a template, a fork, a loaded image) instead of
  copying them, and holds a reference to each until it is dropped. Pages
  only the cell holds (the ones it wrote) are copied as today.
- Before it captures, the kernel makes the cell's page tables its own
  (`PageTable::unshare_tables`), as writes everywhere would but without
  copying a page, so every page another cell can reach counts that
  reference and is copy-on-write everywhere: nobody can write a frame a
  snapshot refers to.
- Resuming maps a referred frame again, copy-on-write if the page is
  writable, instead of copying it into a fresh frame; the cell copies it
  on its first write, as a fork does.
- A snapshot that leaves the kernel stands alone: `Snapshot::export`
  writes the contents of the frames it refers to in their place. The
  exported bytes are the format of today (version 7, unchanged), and
  `from_bytes` refuses a record that refers to a frame, so physical
  addresses never come in from outside.
- `PageTable::map` maps a writable page with `COPY_ON_WRITE` in its flags
  copy-on-write, as `protect` already does.

### Out of scope

- A snapshot's files section still encodes the files it shares with the
  template (the laptop's 64 KiB shared folder is most of what is left of
  an agent's 82 KB snapshot); sharing file blocks is another change.
- An exported snapshot carrying a dependency on its template's own
  snapshot (instead of the template's pages) needs an identity for
  snapshots and a way to ship them together: not before snapshots leave
  the kernel for real (design D2).

## Capabilities

### Modified Capabilities

- `snapshots`: a snapshot refers to the frames its cell shares instead of
  copying them, while it stays in the kernel; an exported snapshot stands
  alone.

## Impact

- `crates/rhizo-mm` (`cow`: `unshare_tables`; `paging::map`),
  `crates/rhizo-snapshot` (capture, restore, export, release),
  `kernel/src/cell.rs` (suspend, resume, lazy pages),
  `kernel/src/control.rs`, `kernel/src/workloads.rs`, `kernel/src/tests.rs`.
- Laptop: `laptop.memory_bytes` 61.1 MB to 7.5 MB (AArch64) and 60.4 MB to
  6.6 MB (x86_64); a suspended agent's snapshot 1.28 MB to 82 KB;
  `laptop.suspend_us` about 5 times faster.
- `cell.snapshot_bytes` and the density figure's `suspended_bytes` shrink
  by the loaded image's pages, which they now share.
- The API changes: `capture` takes the frames and a release function,
  `as_bytes`/`into_bytes` become `heap_bytes` and `export`.
