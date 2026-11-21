# Spec Delta

## ADDED Requirements

### Requirement: Hardware capabilities in the auxiliary vector
On AArch64 the auxiliary vector of every cell SHALL carry `AT_HWCAP` with
`HWCAP_FP`, `HWCAP_ASIMD` and `HWCAP_CPUID`, and no capability a cell
cannot use. On x86_64 it SHALL stay without `AT_HWCAP`: programs read the
CPU with `CPUID`.

#### Scenario: A program checks its capabilities
- **WHEN** a static glibc program reads `getauxval(AT_HWCAP)` in a cell on AArch64
- **THEN** it finds FP, Advanced SIMD and `HWCAP_CPUID`

### Requirement: ID registers read at EL0
A read of an AArch64 ID register by cell code (`MRS` with op0 3, op1 0,
CRn 0, CRm 0 to 7) SHALL be answered by the kernel as Linux answers it:
`MIDR_EL1` and `REVIDR_EL1` as the CPU has them, `MPIDR_EL1` with only
bit 31 set, every other register as zero; the cell resumes after the
instruction.

#### Scenario: A program reads the ID registers
- **WHEN** a static glibc program reads `MIDR_EL1`, `MPIDR_EL1` and `ID_AA64ISAR0_EL1` in a cell on AArch64
- **THEN** it finds a non-zero `MIDR_EL1` and `MPIDR_EL1` equal to bit 31, prints what it prints on Linux and exits with 0

#### Scenario: OpenBLAS picks its kernels
- **WHEN** numpy loads OpenBLAS in a cell on AArch64, which reads `MIDR_EL1` when `HWCAP_CPUID` is set and otherwise opens files under `/proc` and `/sys`
- **THEN** it reads the register and computes, and does not fault

#### Scenario: Any other undefined instruction
- **WHEN** cell code runs an undefined instruction that is not such a read, or writes an ID register
- **THEN** the cell ends with a fault, as before
