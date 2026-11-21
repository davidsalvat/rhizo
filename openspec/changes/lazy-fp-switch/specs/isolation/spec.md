## ADDED Requirements

### Requirement: FP/SIMD registers hold nothing of another thread
While cell code runs, the CPU's FP/SIMD registers SHALL hold only the
running thread's own state or the state a program starts with, never
another thread's or cell's, even before the thread's first FP/SIMD
instruction, so that code run speculatively before an FP/SIMD trap
cannot read them (LazyFP, CVE-2018-3665). This SHALL hold on both
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
