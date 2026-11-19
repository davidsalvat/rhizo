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
];
