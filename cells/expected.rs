// What each test cell prints and how it ends. Shared by the in-kernel tests
// (`kernel/src/tests.rs`) and the differential check on a Linux host
// (`xtask`), so both compare against the same expectations.

/// How a test cell ends.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// `exit_group(status)`.
    Exit(i32),
    /// Killed by a fault (on Linux: `SIGSEGV`).
    Fault,
}

/// `(program, standard output, ending)` for every test cell.
#[allow(dead_code)]
pub const PROGRAMS: &[(&str, &str, Ending)] = &[
    ("hello", "hello from a cell\n", Ending::Exit(0)),
    ("exit3", "", Ending::Exit(3)),
    (
        "abi",
        "brk ok\nmmap ok\nefault ok\nenosys ok\nmisc ok\n",
        Ending::Exit(0),
    ),
    ("fault_null", "", Ending::Fault),
    ("fault_kernel", "", Ending::Fault),
    ("chatter", "x\nx\nx\n", Ending::Exit(0)),
    ("sleeper", "slept ok\n", Ending::Exit(0)),
    ("same_addr", "own value ok\n", Ending::Exit(0)),
    ("peek", "", Ending::Fault),
    ("yielder", "", Ending::Exit(0)),
    ("ballast", "filled\nchecked\n", Ending::Exit(0)),
    ("forkme", "ready\nown ok\n", Ending::Exit(0)),
    ("diary", "ready\nown ok\n", Ending::Exit(0)),
    ("filer", "files ok\n", Ending::Exit(0)),
    (
        "libc_hello",
        "hello from libc, 1 argument, first libc_hello\n\
         sorted: min 0 max 1008 weighted sum 336699270\n\
         strtod 3.141593, scaled 3.142e+10, 0.448799\n\
         0314.159|rhizo |beef (20 chars)\n",
        Ending::Exit(0),
    ),
    ("libc_alloc", "allocator checksum 154285459\n", Ending::Exit(0)),
    ("libc_float", "fp ok: 20100.0\n", Ending::Exit(0)),
    (
        "libc_files",
        "cwd ends in /work\n\
         notes: 100 lines, 2200 bytes, digest 716f37cd630517f4, regular 1\n\
         pread: f2 XYZ 0e\n\
         hole: 0 0 !, size 30001\n\
         dup offset: 2\n\
         truncated to 10\n\
         appended to 14\n\
         exclusive: -1 errno 17\n\
         missing: -1 errno 2\n\
         not a dir: -1 errno 20\n\
         access: 0, x -1 errno 13\n\
         .: data.bin sub\n\
         sub: moved.txt\n\
         rmdir non-empty: -1 errno 39\n\
         after unlink: \"line 000\", stat -1 errno 2\n\
         .:\n\
         files ok\n",
        Ending::Exit(0),
    ),
    (
        "libc_warm",
        "warm\nprimes below 2000000: 148933, below 1000: 168\n",
        Ending::Exit(0),
    ),
    (
        "threads",
        "tids ok\nfutex ok\nmutex ok\nping-pong ok\nregs ok\n",
        Ending::Exit(0),
    ),
    (
        "libc_threads",
        "ready\n\
         pool: 500500 2001000 4501500\n\
         queue: 400 items, sum 240200\n\
         mutex: 400 increments\n\
         fp: 210.0 20190.0\n\
         threads: 13, distinct ids yes\n",
        Ending::Exit(0),
    ),
    (
        "counter",
        "step 07 digest 6b4e24cdafe14e6b\nstep 15 digest c93f1c055e5410cf\nstep 23 digest 65632246df41d107\nstep 31 digest a7c1ff878b0627df\nstep 39 digest 9a1ea881edd8f792\n",
        Ending::Exit(0),
    ),
    // Hostile cells (spec `isolation`) whose answers Linux gives too.
    (
        "hostile_ptrs",
        "efault ok\nkernel ranges ok\nhuge ranges ok\nthread pointer ok\n",
        Ending::Exit(0),
    ),
    ("poke_kernel", "", Ending::Fault),
    ("scrub", "ready\nzero ok\n", Ending::Exit(0)),
    ("rewrite", "ready\nown ok\n", Ending::Exit(0)),
];

/// Hostile test cells run only in the kernel (spec `isolation`): random
/// system calls, or taking all memory, are not for the host that runs the
/// differential check.
#[allow(dead_code)]
pub const KERNEL_ONLY: &[(&str, &str, Ending)] = &[
    ("fuzz", "fuzz ok\n", Ending::Exit(0)),
    ("hog", "exhausted\n", Ending::Exit(0)),
    ("namer", "names full\n", Ending::Exit(0)),
];

/// Test cells that never end on their own (they are killed by the tests):
/// not run by the differential check.
#[allow(dead_code)]
pub const ENDLESS: &[&str] = &["spin", "mapper", "idle", "storm"];

/// MicroPython (`cells/python`, built only when its source is given): the
/// command it runs, with `rhizo_demo.py` in `lib/` relative to the working
/// directory, and what it prints. The module pauses after `warm`, where a
/// host may freeze it as a template.
#[allow(dead_code)]
pub const PYTHON_COMMAND: &str =
    "import sys; sys.path.insert(0, 'lib'); import rhizo_demo; rhizo_demo.main()";
#[allow(dead_code)]
pub const PYTHON_STDOUT: &str = "warm\n\
     primes below 10000: 1229, last 9973; top words: agent=70, kernel=69, fork=67\n\
     python ok\n";
