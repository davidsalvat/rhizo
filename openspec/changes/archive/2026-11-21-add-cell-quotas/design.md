# Design

## D1. Memory: count frames taken, credit frames given back

Every place the kernel takes frames for a cell goes through one adapter,
`rhizo_cell::quota::Metered`, around the kernel's allocator: system calls
(`mmap`, `brk`, `munmap`, `mprotect`), the pager (lazy pages, stack
growth), the fault path, spawn and resume.
`Metered` refuses an allocation that would take the cell's count past its
quota (a 2 MiB page counts 512 frames), and credits every frame the cell
gives back, saturating at zero. The count and the limit live in the
kernel's record of the live cell (`Live::frames`, a `quota::Meter`), so a
meter is one pointer: a system call builds it with one store, and the
limit is set when the quota is.

Copy-on-write copies are not metered: a copy replaces the cell's reference
to a shared frame (or table) by a frame of its own, so the meter would
charge one frame and credit one back, whichever cell writes. Leaving them
out changes no count; it only drops the refusal of a copy by a cell
exactly at its limit, which would have been credited at once. (Splitting
a shared 2 MiB page whose copy cannot be had takes one page table more
than it gives back: one frame per such fault, bounded by what the cell
inherited.) Metering them cost 28 instructions per copy-on-write fault.

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

File data stays out: it has its own bound per cell (`rhizo_fs::DATA_MAX`
of data, 4096 names), and counting it would need hooks in `rhizo-fs` on
every free. A cell's memory is therefore bounded by its quota plus
`DATA_MAX`; a host that needs a tighter bound waits for file data to be
metered (its own change). A
hole or a page past the end of a mapped file is not file data: it is the
cell's alone, never kept by the page cache, so it comes from the cell's
frames and is counted (finding S20).

## D2. CPU time: per slice, checked at its end

For a cell with a CPU quota, `run_slice` subtracts the counter from
`Live::cpu_ticks` as the slice starts and adds it back when the cell
comes back (yield, block, preemption), so nothing stays in a register
across the slice; the ticks are then added to `Cell::cpu_ns`, system calls
included. A cell past its quota ends there with `Ending::OutOfCpu`, and
`cell N killed: CPU time quota used up` goes to its output. A cell
overruns its quota by at most one slice (10 ms) plus one system call.

A cell without a CPU quota is not timed: the cost is one test of its quota
at each end of the slice (none on the system-call path), where counting
every cell cost 16 instructions per switch between cells, and converting
every slice to nanoseconds about 40. So CPU time counts while the cell has
a quota: a quota the host sets later counts from then, and a snapshot of
a cell without one records the time counted before (none, or what it used
under an earlier quota).

Alternative: a timer at the exact deadline. Rejected: the preemption tick
already bounds the overrun, and a second timer per CPU is more code.

### What quotas cost per switch

On x86_64 (`-icount`), a switch between cells costs 9 instructions more
than without quotas (871 to 880, +1.0 %): 6 for metering the system call
each switch makes (a meter pointer in the call's context and one in its
pager) and 3 for the CPU-quota tests at the ends of the slice. A system
call costs 6 more (330 to 336). On AArch64 a switch costs 13 more (815 to
828). The x86_64 switch baseline moves to 880 with this change.

Rejected ways to cut it: a per-CPU pointer to the running cell's meter,
read by a zero-sized allocator (hidden global state and `unsafe`, against
the minimal-code and `unsafe` rules), and not metering frames the pager
takes during system calls (stack growth unmetered, the hole quotas close).

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
