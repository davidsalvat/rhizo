# Spec Delta

## ADDED Requirements

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
