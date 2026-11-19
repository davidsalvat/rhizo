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
    (
        "counter",
        "step 07 digest 6b4e24cdafe14e6b\nstep 15 digest c93f1c055e5410cf\nstep 23 digest 65632246df41d107\nstep 31 digest a7c1ff878b0627df\nstep 39 digest 9a1ea881edd8f792\n",
        Ending::Exit(0),
    ),
];

/// Test cells that never end on their own (they are killed by the tests):
/// not run by the differential check.
#[allow(dead_code)]
pub const ENDLESS: &[&str] = &["spin", "mapper"];
