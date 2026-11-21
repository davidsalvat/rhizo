# Design

## D1. Refuse the reference, copy the page

When a frame's count is full, the caller that wanted one more reference
gets a private copy of the page instead. Alternatives:

| Option | Panic gone | Leak | Cost |
|---|---|---|---|
| **`try_share`, copy on refusal (chosen)** | yes | no | one copy per mapping past 65535 |
| Wider counts (`u32`) | only in practice: 2^32 mappings need 32 GiB of tables | no | 2 more bytes per frame of RAM, always |
| Saturate and pin (as Linux's `refcount_t`) | yes | the frame, forever: a cell could leak 4 KiB per 65535 calls, without end | none |
| Refuse the mapping (`ENOMEM`) | yes | no | a program mapping one library page that often fails, unlike on Linux |

The copy is what the cell would have got from an empty cache, so nothing
changes for it; its frame is taken like any other the cell takes.

## D2. Check and add at once

`try_share` checks and adds in one step, under one acquisition of the
lock of the counts: a separate "can share" query followed by `share`
could be overtaken by another CPU sharing the same frame in between. The
default implementation (`share`, then `true`) keeps every existing
`Frames` as it is; only the kernel's frames and the host test frames
(`CountingFrames::max_refs`) bound their counts.
