## MODIFIED Requirements

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

## ADDED Requirements

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
