# Spec Delta

## MODIFIED Requirements

### Requirement: Memory quota per cell
The host, or a cell's parent within its own budget, SHALL be able to set
how much memory a cell may take (its pages and page tables, and what
requirement "Everything a cell takes is charged" of `cell-hierarchy`
adds). An allocation past it SHALL fail as when memory runs out (`mmap`
and `brk` answer `-ENOMEM`), and SHALL NOT take memory from other cells or
the kernel.

#### Scenario: Hog with a quota
- **WHEN** a cell with an 8 MiB memory quota takes memory until refused, next to a cell without quota that asks for 16 MiB
- **THEN** the first is refused with `-ENOMEM` having taken at most 8 MiB, the other gets all it asked for, and every frame comes back when both end

#### Scenario: Holes of a mapped file count
- **WHEN** a cell with an 8 MiB memory quota maps a one-byte file of its own 1 MiB at a time until refused, next to a cell without quota that asks for 16 MiB
- **THEN** the pages past the file's end count in its quota: it is refused with `-ENOMEM` having taken at most 8 MiB, and the other gets all it asked for

### Requirement: CPU time quota per cell
The host, or a cell's parent within its own budget, SHALL be able to set
how much CPU time a cell may use, its system calls included, counted from
when the quota is set and across suspend and resume. A cell past it SHALL
be ended at the end of the time slice that used it up, with a report in
its output.

#### Scenario: Spinning past its quota
- **WHEN** a cell with a 30 ms CPU quota spins forever next to another cell
- **THEN** it ends with `cell N killed: CPU time quota used up` and the other cell runs to its end

### Requirement: Descriptor and socket quotas per cell
The host, or a cell's parent within its own budget, SHALL be able to set
how many descriptors (at most 1024) and sockets a cell may hold; without a
quota, a cell SHALL hold at most 256 sockets. A new descriptor past the
first SHALL fail with `-EMFILE`, a new socket past the second with
`-ENFILE`, and `getrlimit`/`prlimit64` SHALL report the cell's quotas on
descriptors, memory and CPU time.

#### Scenario: A bounded share of sockets by default
- **WHEN** a cell for which the host set no quota, and its fork, make sockets until refused
- **THEN** each gets 256 sockets then `-ENFILE`, so no cell holds more than an eighth of the kernel's 4096 sockets with the server ends of its own connections

#### Scenario: Opening until refused
- **WHEN** a cell with quotas of 16 descriptors and 3 sockets makes sockets until refused, closes them, then opens files until refused
- **THEN** it gets 3 sockets then `-ENFILE`, 13 files then `-EMFILE`, and `RLIMIT_NOFILE` reads 16

## ADDED Requirements

### Requirement: No storage channel between domains
Apart from elapsed time, nothing a cell observes SHALL depend on what
cells of another domain do, as long as both domains' roots are reserved.
This covers the results and error codes of system calls, file contents
and names, socket addresses and ports, identifiers, memory figures, and
which calls block.

#### Scenario: Differential colluders
- **WHEN** a receiver cell in one domain runs a fixed probe (allocate to its quota, write files, open sockets, bind and connect loopback ports, take ephemeral ports, connect outside, map file pages, read `sysinfo` and `getpid`, make children), once alone and once while a sender in another domain does each of these as hard as it can
- **THEN** the receiver's output, apart from times, is byte for byte the same both times

### Requirement: Timing channels listed and measured
`docs/security.md` SHALL list every kernel resource shared between
domains with its storage and timing exposure and its mitigation, and
SHALL give the measured bandwidth of a CPU-contention channel between two
domains. No timing channel SHALL carry a handle, a socket or network
reach.

#### Scenario: Measured channel
- **WHEN** the benchmark runs a sender and a receiver in two domains on one CPU
- **THEN** it reports `covert.cpu_bps`, and the inventory in `docs/security.md` cites it

### Requirement: Frames shared across domains stay held
A template whose copies live in more than one domain SHALL keep its
references to its frames after it is dropped, until its last copy ends,
so that whether a cell's write to a shared page copies it never depends
on another domain.

#### Scenario: Template dropped under two domains
- **WHEN** two domains run copies of one template, the template is dropped, and one domain's copies end
- **THEN** each write of the other domain's copies to a page they shared still makes a private copy

### Requirement: File pages kept per domain
Pages of mapped files SHALL be shared only between cells of one domain:
the page cache SHALL be keyed by domain and by contents, bounded and
emptied per domain, and charged to the domain's root.

#### Scenario: Flushing another domain
- **WHEN** a cell maps more file pages than its domain may keep, while a cell of another domain maps a library page again and again
- **THEN** the second cell's mappings stay cache hits, and its page is never copied again

### Requirement: Exclusive CPU sets per domain
The host MAY give a root an exclusive set of CPUs: the cells of its domain
SHALL then run only on those CPUs and no other cell SHALL run there, so
CPU scheduling and core-private caches are not shared with other
domains. The boot CPU SHALL never be exclusive, and the sets of all roots
SHALL leave at least one CPU shared.

#### Scenario: Contention channel closed
- **WHEN** a sender and a receiver of the CPU-contention channel run in two domains with exclusive sets of one CPU each
- **THEN** `covert.cpu_bps` with sets is reported next to the figure without, and the receiver's timings do not follow the sender's bits
