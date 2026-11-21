## MODIFIED Requirements

### Requirement: FP/SIMD registers hold nothing of another thread
While cell code runs, the CPU's FP/SIMD registers SHALL hold only the
running thread's own state or the state a program starts with, never
another thread's or cell's, even before the thread's first FP/SIMD
instruction, so that code run speculatively before an FP/SIMD trap cannot
read them (LazyFP, CVE-2018-3665). This SHALL hold on both
architectures and every CPU, including threads of one cell on several
CPUs.

#### Scenario: Registers after another cell
- **WHEN** a cell fills every FP/SIMD register with its own values and yields, and a cell that never uses FP/SIMD runs next on that CPU
- **THEN** the CPU's FP/SIMD registers, read by the kernel after that cell's turn, hold the initial state

#### Scenario: A hostile reader
- **WHEN** a cell that only reads its FP/SIMD registers (its first FP/SIMD instruction a read) runs many times next to cells and threads holding distinct values in theirs, on 1 and on 4 CPUs
- **THEN** it only ever finds the initial state, and every other cell and thread finds its own values

#### Scenario: Suspends, resumes and forks
- **WHEN** a cell whose threads hold values in their FP/SIMD registers is suspended and resumed (eagerly and lazily, sometimes twice before it runs again) and forked while it runs
- **THEN** it and its forks find their own values, and so does every other cell

## ADDED Requirements

### Requirement: x87 pointers hold nothing of another thread
On x86_64, the x87 last-instruction opcode, instruction pointer and data
pointer a cell thread finds SHALL be its own or kernel values, never
another thread's or another cell's, including on CPUs whose `FXRSTOR` does
not load them: AMD and Hygon CPUs without `XSAVEERPTR`, and QEMU's
emulation. The kernel SHALL decide at start whether the CPU needs this,
and SHALL add no work to a thread's FP/SIMD load on a CPU that does not.

#### Scenario: x87 pointers after a switch
- **WHEN** an x87 load runs on a CPU and another thread's FP/SIMD state is then loaded there, and that thread reads its x87 environment
- **THEN** the instruction and data pointers it reads are not those of the earlier load

#### Scenario: Only where needed
- **WHEN** the kernel starts on an x86_64 CPU
- **THEN** it scrubs the x87 pointers before each FP/SIMD load exactly when the CPU is AMD or Hygon without `XSAVEERPTR`, or QEMU's emulation
