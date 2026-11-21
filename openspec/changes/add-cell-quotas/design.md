# Design

## D1. Memory: count frames taken, credit frames given back

Every place the kernel takes frames for a cell goes through one adapter,
`rhizo_cell::quota::Metered`, around the kernel's allocator: system calls
(`mmap`, `brk`, `munmap`, `mprotect`), the pager (lazy pages,
copy-on-write copies, stack growth), the fault path, spawn and resume.
`Metered` refuses an allocation that would take the cell's count past its
quota (a 2 MiB page counts 512 frames), and credits every frame the cell
gives back, saturating at zero. The count lives in the kernel's record of
the live cell (`Live::frames`), shared by the meters of one call.

Frames shared after a fork stay counted to the cell that took them. A
child is credited when it gives back a page it inherited (saturating at
zero), so a cell holds at most its quota plus what it inherited, which was
counted against its parent or template. Pages already the cell's that come
back while it is suspended, forked or frozen (the rest of a lazy resume)
are counted without being refused: they were the cell's before. The
frames the dynamic loader takes to start a dynamically linked program are
not counted yet (it takes the kernel's allocator directly); the host sets
quotas after the spawn, and the credit for giving them back saturates.

What happens past the quota is what happens when memory runs out today:
`mmap` and `brk` answer `ENOMEM`; a page fault that needs a frame (a
copy-on-write copy, a lazy page, stack growth) cannot be served and the
cell is killed with its fault report. The kernel's own reserve still
applies on top.

Alternative: count pages mapped (resident set) by walking the tables.
Rejected: a walk per call is slow, and a fork would have to charge every
shared page to the child, which is what makes forks cheap.

Alternative: an owner per frame, credited to whoever took it. Rejected
for now: 4 bytes per frame of global state for a precision the bound above
already gives.

File data stays out: it has its own bound per cell (64 MiB of data, 4096
names), and counting it would need hooks in `rhizo-fs` on every free. A
hole or a page past the end of a mapped file is not file data: it is the
cell's alone, never kept by the page cache, so it comes from the cell's
frames and is counted (finding S20).

## D2. CPU time: per slice, checked at its end

`run_slice` knows when it picked the cell; when the cell comes back
(yield, block, preemption) the time since then is added to
`Cell::cpu_ns`, system calls included. A cell past its quota ends there
with `Ending::OutOfCpu`, and `cell N killed: CPU time quota used up` goes
to its output. A cell overruns its quota by at most one slice (10 ms) plus
one system call; one clock read per slice is the whole cost (none on the
system-call path).

Alternative: a timer at the exact deadline. Rejected: the preemption tick
already bounds the overrun, and a second timer per CPU is more code.

## D3. Quotas are cell state

`Quota` and `cpu_ns` live in `rhizo_cell::Cell`, plain data:

- a fork or a copy of a template gets the cell's quotas and none of its
  usage (CPU time 0, its root as its only frame), like a child process's
  limits and CPU time;
- a snapshot (format version 8) keeps the quotas and the CPU time used, so
  suspending and resuming a cell does not give it new time;
- the limits on descriptors and sockets are applied to the cell's file
  tables (`Fs::set_limits`) whenever a live cell is made from a `Cell`
  (spawn, fork, template copy, resume), only if they differ (setting them
  copies tables shared copy-on-write);
- `getrlimit`/`prlimit64` report them, so programs that size themselves
  from `RLIMIT_NOFILE` or `RLIMIT_AS` see the truth; a cell cannot raise
  them (`setrlimit` does nothing).

## D4. Descriptors and sockets in `rhizo-fs`

`Tables` keeps two limits, not encoded. New descriptors are looked for
below the descriptor limit (`EMFILE` past it, `EBADF` for `dup2` past it).
A new socket descriptor is refused with `ENFILE` once the cell holds its
quota of socket descriptions (a duplicate descriptor shares its socket and
is not counted again). The kernel's network socket made for a refused
call is released at once, as when no descriptor is free today.

## D5. A default socket quota (finding S21)

The kernel holds at most 4096 sockets for all cells. Without a quota a
cell could hold about half: 1024 connections to its own listener, each
with the server end the kernel makes at `connect`. A new cell therefore
gets `Quota::DEFAULT`: no limit on memory or CPU time, 1024 descriptors,
256 sockets. With the server ends of its own connections, a cell holds at
most 512 sockets, an eighth of the pool; a compile-time check in the
kernel keeps it so. The file tables start with the same limit, so making
a live cell from a new `Cell` copies no tables. The host may set any
quota, `Quota::NONE` included.

Alternative: count the kernel's sockets per cell in `rhizo-net`, server
ends included. Rejected: an owner per socket, fork and snapshot records
to carry it, for a bound the descriptor count already gives within a
factor of two.
