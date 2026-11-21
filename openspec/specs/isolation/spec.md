# isolation Specification

## Purpose
What keeps one cell from reaching the kernel, another cell or memory it was not given, under hostile input: the threat model reviewed in `docs/security.md` and the checks that hold it.

## Requirements

### Requirement: The kernel half is out of every cell's reach
A cell SHALL NOT read, write, map, unmap or change the permissions of
kernel memory, by its own instructions or through any system call. A cell's
addresses end at `USER_END` (`0x7fff_ffff_f000`, as Linux's
`TASK_SIZE_MAX`): every copy between a cell and the kernel SHALL refuse a
range that reaches past it, whatever the page tables say, and no mapping
call SHALL change an entry for an address past it.

#### Scenario: Kernel pointer passed to a call
- **WHEN** a cell passes a kernel, non-canonical or last-page address as the buffer, path or result of any call that takes one
- **THEN** the call returns `-EFAULT` and kernel memory is unchanged

#### Scenario: Kernel range passed to mprotect or munmap
- **WHEN** a cell asks `mprotect` for any permissions, or `munmap`, on a range in the kernel half
- **THEN** `mprotect` returns `-ENOMEM`, `munmap` returns `-EINVAL`, and no kernel page-table entry changes

#### Scenario: Writing kernel memory
- **WHEN** cell code writes to a kernel address
- **THEN** the cell is killed with a fault report naming the address and a write, and the kernel keeps running cells

### Requirement: Hostile calls get answers, not crashes
Whatever call number and arguments a cell passes, the kernel SHALL answer
with a value or a negated Linux `errno`, SHALL NOT panic, and SHALL leave
kernel memory unchanged; once the cell ends, every frame it held SHALL be
free again. No count of references to a frame that a cell can grow
(mapping one file page again and again) SHALL overflow or panic: past its
bound, the cell gets a copy of the page instead.

#### Scenario: Random calls
- **WHEN** a cell makes 20000 system calls with numbers and arguments from a fixed-seed generator, next to another cell
- **THEN** every result is a value or an errno, both cells finish, and running it again leaves memory as it was

#### Scenario: One page mapped without end
- **WHEN** a cell maps the first page of a file of its own 70000 times, next to another cell
- **THEN** every mapping reads the file's bytes, both cells finish, the kernel keeps running, and every frame comes back

### Requirement: State the kernel loads stays the cell's
The kernel SHALL refuse values that would fault in the kernel itself when
it enters the cell: a thread pointer past `USER_END` (`arch_prctl` answers
`-EPERM`), an executable segment past `USER_END`, a first instruction (an
executable's entry point, or its interpreter's at its load address) at or
past `USER_END`, and a snapshot whose program counter (or, on x86_64,
thread pointer) is past `USER_END` or whose `MXCSR` has reserved bits set.

#### Scenario: Kernel thread pointer
- **WHEN** a cell on x86_64 calls `arch_prctl(ARCH_SET_FS)` with a kernel or non-canonical address
- **THEN** it receives `-EPERM` and its thread pointer is unchanged

#### Scenario: Tampered snapshot
- **WHEN** the bytes of a snapshot hold a kernel program counter
- **THEN** reading them fails with a format error and nothing is resumed

#### Scenario: Entry point past the cell
- **WHEN** a program, or the interpreter it names, has an entry point that lands at or past `USER_END`
- **THEN** starting it fails with `BadEntry` and every frame taken is given back

### Requirement: Bounded kernel work per call
The kernel runs a system call to its end without preemption, so no call
SHALL take time proportional to the size of an empty range: walks over a
range SHALL skip unmapped space one empty table entry at a time, and
`mprotect` SHALL stop at the first page it cannot bring in. `mmap` and
`brk` SHALL refuse memory that cannot be had before zeroing any of it.

#### Scenario: Call storm
- **WHEN** a cell calls `munmap` and `mprotect` over terabytes of unmapped space without pause, next to another cell
- **THEN** the other cell runs to its end

### Requirement: Bounded kernel memory per cell
The kernel memory a cell's actions take SHALL be bounded or counted like
its pages: at most 4096 files and directories per cell, each created only
when memory for it can be had (`-ENOSPC` otherwise); a `getdents64` answer
of at most 64 KiB; at most 1 MiB of captured output; and one console
report per unknown call number below 1023, and a single one for all larger
numbers.

#### Scenario: Endless names
- **WHEN** cells make directories with the longest names until refused, while another cell takes all memory it can
- **THEN** each is refused with `-ENOSPC`, all end, the kernel keeps running, and every frame comes back

### Requirement: No data crosses cells through memory
Memory given to a cell (a fresh mapping, its heap, the private copy of a
forked page, a resumed page) SHALL hold nothing written by another cell,
and what an ended cell mapped SHALL be mapped for no other cell.

#### Scenario: Recycled frames
- **WHEN** a cell fills 16 MiB with a pattern and exits, then a new cell, a forked child or a resumed cell maps 16 MiB and grows its heap
- **THEN** every byte it reads there is zero

#### Scenario: Shared page made writable
- **WHEN** copies of a forked cell each make a read-only page they share writable with `mprotect` and write to it
- **THEN** each reads back its own write, and the others' pages are unchanged

#### Scenario: After the end
- **WHEN** a cell reads the address where an ended cell had a mapping
- **THEN** it is killed with a fault report

### Requirement: Random bytes are unpredictable
The bytes the kernel gives a cell (`getrandom` and `AT_RANDOM`) SHALL come
from a ChaCha20 generator with fast key erasure, seeded from the CPU's
hardware random source, which enters the kernel only through
`rhizo_hal::Entropy`. When the CPU has no hardware source the kernel SHALL
say at start that `getrandom` is predictable.

#### Scenario: Generator checked against the standard
- **WHEN** the ChaCha20 block function is run on the test vectors of RFC 8439
- **THEN** it produces the RFC's key stream

#### Scenario: Earlier output stays secret
- **WHEN** the state of a generator is read after it answered a request
- **THEN** that state does not produce the bytes already answered

### Requirement: Random streams are private to each cell
Each cell SHALL draw from its own generator. A fork, a copy of a template
and a resumed snapshot SHALL NOT draw what any other cell draws: no
generator state is copied into a child or kept in a snapshot.

#### Scenario: Copies of one cell
- **WHEN** a cell that drew random bytes is forked twice, frozen into a template copied twice, and suspended into a snapshot resumed twice (eagerly and lazily), and each copy then calls `getrandom`
- **THEN** every copy, and every independent cell, draws different bytes

### Requirement: The CPU refuses kernel accesses to cell pages
On CPUs that have them, the kernel SHALL run with SMEP and SMAP (x86_64)
or PAN (AArch64) on, so a kernel load, store or instruction fetch at a
cell's address faults. The kernel SHALL reach cell memory only through
the user-memory copies of `rhizo_cell::memory`, which use the direct map.

#### Scenario: Stray kernel read of a user page
- **WHEN** kernel code reads a page mapped for a cell, by its user address
- **THEN** the CPU raises a permission fault in kernel mode instead of returning the data

#### Scenario: Stray kernel jump into a user page
- **WHEN** kernel code calls a page mapped for a cell
- **THEN** the CPU raises a permission fault on the instruction fetch

### Requirement: A cell's flags never reach kernel code
Flags a cell sets for itself that change how the kernel's code runs
(x86_64 `RFLAGS.DF` and `RFLAGS.AC`) SHALL be cleared on every entry to
the kernel from the cell, by system call, exception or interrupt, and
SHALL be the cell's again when it resumes.

#### Scenario: Interrupted with the direction flag set
- **WHEN** a cell spins with the direction and alignment-check flags set and is interrupted many times, next to other cells
- **THEN** the kernel runs with both clear after every interruption, the cell finds its direction flag as it set it, and the other cells finish with their expected output

### Requirement: Snapshots from outside are contained
Snapshot bytes SHALL be treated as written by anyone: any bytes SHALL be
refused with a format error, or resume into a cell that changes no kernel
memory or kernel page-table entry, maps only user pages below `USER_END`
and never writable and executable at once, starts every thread below
`USER_END`, has files whose every operation answers instead of panicking,
and gives every frame back when it ends.

#### Scenario: Tampered or random snapshot
- **WHEN** a valid snapshot with bytes changed or cut, or random bytes behind a valid header, are read and resumed
- **THEN** they are refused, or the resumed cell holds the properties above

### Requirement: A broken device cannot widen what the kernel maps
The kernel SHALL map a virtio-pci structure only inside a memory BAR whose
size is a power of two and whose end does not wrap past the top of the
address space; a function that reports otherwise SHALL be refused, not
followed.

#### Scenario: Changed configuration space
- **WHEN** a virtio network function's configuration space has any dwords changed and any BAR sizes
- **THEN** setting it up does not panic, and every structure it reports lies inside one of its BARs

### Requirement: Counters a cell may read
A cell SHALL be able to read the CPU's counter directly, without a system
call, on every CPU it runs on: the virtual counter and its frequency
(`CNTVCT_EL0`, `CNTFRQ_EL0`) on AArch64 and the time-stamp counter
(`rdtsc`) on x86_64, as on Linux. The physical counter and the timers on
AArch64 SHALL stay the kernel's. The counter is not cell state: it keeps
running while a cell is suspended, as `CLOCK_MONOTONIC` does, and a
resumed or forked cell reads the machine's counter.

#### Scenario: Reading the counter
- **WHEN** a cell reads the counter twice
- **THEN** both readings are nonzero, the second is not smaller, and the cell keeps running

### Requirement: x86_64 cells do not map the kernel
On x86_64 CPUs open to Meltdown (all but AMD, Hygon and Intel CPUs that
report `RDCL_NO`), the tables a cell runs on SHALL map nothing of the
kernel half but the entry pages: entry code, per-CPU data, GDT, TSS, IDT
and trap stacks. The kernel SHALL switch to its own tables on every entry
from a cell and back on every return (KPTI).

#### Scenario: What a cell's tables map
- **WHEN** KPTI is on and the kernel half of a cell's tables is walked
- **THEN** the system-call entry is mapped and executable, and the direct map, the heap, the kernel's code and data and its page tables are not

#### Scenario: Same behavior both ways
- **WHEN** cells make system calls, fault, are forked and switch with KPTI off and then on
- **THEN** they behave the same, and every in-kernel test passes with KPTI on

### Requirement: AArch64 cells cannot reach the kernel half
On AArch64 the kernel SHALL set `TCR_EL1.E0PD1` on every CPU that has
FEAT_E0PD, so cell accesses to the kernel half fault in constant time, and
SHALL say at start when the CPU has neither E0PD nor `CSV3`.

#### Scenario: Kernel address from a cell
- **WHEN** a cell reads a kernel address
- **THEN** it is killed with a fault report naming the address

### Requirement: Memory quota per cell
The host SHALL be able to set how much memory a cell's pages and page
tables may take. An allocation past it SHALL fail as when memory runs out
(`mmap` and `brk` answer `-ENOMEM`), and SHALL NOT take memory from other
cells or the kernel.

#### Scenario: Hog with a quota
- **WHEN** a cell with an 8 MiB memory quota takes memory until refused, next to a cell without quota that asks for 16 MiB
- **THEN** the first is refused with `-ENOMEM` having taken at most 8 MiB, the other gets all it asked for, and every frame comes back when both end

#### Scenario: Holes of a mapped file count
- **WHEN** a cell with an 8 MiB memory quota maps a one-byte file of its own 1 MiB at a time until refused, next to a cell without quota that asks for 16 MiB
- **THEN** the pages past the file's end count in its quota: it is refused with `-ENOMEM` having taken at most 8 MiB, and the other gets all it asked for

### Requirement: CPU time quota per cell
The host SHALL be able to set how much CPU time a cell may use, its
system calls included, counted from when the quota is set and across
suspend and resume. A cell past it SHALL be ended at the end
of the time slice that used it up, with a report in its output.

#### Scenario: Spinning past its quota
- **WHEN** a cell with a 30 ms CPU quota spins forever next to another cell
- **THEN** it ends with `cell N killed: CPU time quota used up` and the other cell runs to its end

### Requirement: Descriptor and socket quotas per cell
The host SHALL be able to set how many descriptors (at most 1024) and
sockets a cell may hold; without a quota from the host, a cell SHALL hold
at most 256 sockets. A new descriptor past the first SHALL fail with
`-EMFILE`, a new socket past the second with `-ENFILE`, and
`getrlimit`/`prlimit64` SHALL report the cell's quotas on descriptors,
memory and CPU time.

#### Scenario: A bounded share of sockets by default
- **WHEN** a cell for which the host set no quota, and its fork, make sockets until refused
- **THEN** each gets 256 sockets then `-ENFILE`, so no cell holds more than an eighth of the kernel's 4096 sockets with the server ends of its own connections

#### Scenario: Opening until refused
- **WHEN** a cell with quotas of 16 descriptors and 3 sockets makes sockets until refused, closes them, then opens files until refused
- **THEN** it gets 3 sockets then `-ENFILE`, 13 files then `-EMFILE`, and `RLIMIT_NOFILE` reads 16

### Requirement: Quotas are cell state
A cell's quotas SHALL travel with it: a fork and a copy of a template SHALL
have the cell's quotas and none of its usage, and a resumed snapshot SHALL
have the cell's quotas and the CPU time it had used.

#### Scenario: Copies keep the quotas
- **WHEN** a cell with descriptor and socket quotas is forked, frozen and copied, and suspended and resumed, and each copy opens until refused
- **THEN** every copy stops at the same quotas as the cell

### Requirement: FP/SIMD registers hold nothing of another thread
The FP/SIMD state a cell thread finds when it runs SHALL be its own or
kernel values, never another thread's or another cell's. On x86_64 this
covers the x87 last-instruction opcode, instruction pointer and data
pointer, including on CPUs whose `FXRSTOR` does not load them: AMD and
Hygon CPUs without `XSAVEERPTR`, and QEMU's emulation. The kernel SHALL
decide at start whether the CPU needs this, and SHALL add no work to a
thread's FP/SIMD load on a CPU that does not.

#### Scenario: x87 pointers after a switch
- **WHEN** an x87 load runs on a CPU and another thread's FP/SIMD state is then loaded there, and that thread reads its x87 environment
- **THEN** the instruction and data pointers it reads are not those of the earlier load

#### Scenario: Only where needed
- **WHEN** the kernel starts on an x86_64 CPU
- **THEN** it scrubs the x87 pointers before each FP/SIMD load exactly when the CPU is AMD or Hygon without `XSAVEERPTR`, or QEMU's emulation
