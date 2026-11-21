# Spec Delta

## ADDED Requirements

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
