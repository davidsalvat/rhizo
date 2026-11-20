//! In-kernel test runner.
//!
//! The kernel's test build calls [`run`] with its list of test cases. Each case
//! is a plain function; a failing case panics, and the kernel's panic handler
//! reports the failure and stops the virtual machine. Because the name of each
//! case is printed before it runs, a failure is always attributed to a test.

use core::fmt::Write;
use rhizo_hal::{Console, ConsoleWriter, ExitStatus};

/// One in-kernel test case.
#[derive(Debug, Clone, Copy)]
pub struct TestCase {
    /// Name printed in the results.
    pub name: &'static str,
    /// The test body. It fails by panicking.
    pub run: fn(),
}

/// Builds a [`TestCase`] from a function, using the function's name.
#[macro_export]
macro_rules! ktest_case {
    ($f:path) => {
        $crate::ktest::TestCase {
            name: stringify!($f),
            run: $f,
        }
    };
}

/// Runs every test case of every suite, in order, and returns the status to
/// exit the VM with. With `shard = Some((i, n))` only the cases whose
/// position (counting across the suites) is `i` modulo `n` run, so `n` VMs
/// together run each case once.
///
/// Output format, one line per test:
///
/// ```text
/// running 2 tests
/// test first ... ok
/// test second ... ok
/// test result: ok. 2 passed
/// ```
pub fn run(
    console: &mut impl Console,
    suites: &[&[TestCase]],
    shard: Option<(u16, u16)>,
) -> ExitStatus {
    let (i, n) = shard.map_or((0, 1), |(i, n)| (usize::from(i), usize::from(n)));
    let mine = |k: &usize| k % n == i;
    let all = || suites.iter().flat_map(|s| s.iter()).enumerate();
    let total = all().filter(|(k, _)| mine(k)).count();
    let _ = writeln!(ConsoleWriter(console), "running {total} tests");
    for (_, case) in all().filter(|(k, _)| mine(k)) {
        let _ = write!(ConsoleWriter(console), "test {} ... ", case.name);
        (case.run)();
        let _ = writeln!(ConsoleWriter(console), "ok");
    }
    let _ = writeln!(ConsoleWriter(console), "test result: ok. {total} passed");
    ExitStatus::Success
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhizo_hal::testing::RecordingConsole;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Calls of `counted`, which only `all_passing_tests_report_ok_and_success`
    /// runs: tests run in parallel, so a counter shared with other tests
    /// would race.
    static CALLS: AtomicUsize = AtomicUsize::new(0);

    fn counted() {
        CALLS.fetch_add(1, Ordering::SeqCst);
    }

    fn passing() {}

    fn failing() {
        panic!("assertion failed");
    }

    #[test]
    fn all_passing_tests_report_ok_and_success() {
        let before = CALLS.load(Ordering::SeqCst);
        let mut console = RecordingConsole::new();
        let cases = [ktest_case!(counted), ktest_case!(counted)];

        let status = run(&mut console, &[&cases], None);

        assert_eq!(status, ExitStatus::Success);
        assert_eq!(CALLS.load(Ordering::SeqCst) - before, 2);
        assert_eq!(
            console.text(),
            "running 2 tests\n\
             test counted ... ok\n\
             test counted ... ok\n\
             test result: ok. 2 passed\n"
        );
    }

    #[test]
    fn shards_run_every_case_exactly_once() {
        fn a() {}
        fn b() {}
        fn c() {}
        fn d() {}
        fn e() {}
        let first = [ktest_case!(a), ktest_case!(b)];
        let second = [ktest_case!(c), ktest_case!(d), ktest_case!(e)];
        let mut names = Vec::new();
        for i in 0..3 {
            let mut console = RecordingConsole::new();
            assert_eq!(
                run(&mut console, &[&first, &second], Some((i, 3))),
                ExitStatus::Success
            );
            let text = console.text();
            let ran: Vec<&str> = text
                .lines()
                .filter_map(|l| l.strip_prefix("test ")?.strip_suffix(" ... ok"))
                .collect();
            assert!(text.starts_with(&format!("running {} tests\n", ran.len())));
            assert!(text.ends_with(&format!("test result: ok. {} passed\n", ran.len())));
            names.extend(ran.iter().map(|n| n.to_string()));
        }
        names.sort();
        assert_eq!(names, ["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn empty_suite_succeeds() {
        let mut console = RecordingConsole::new();
        assert_eq!(run(&mut console, &[], None), ExitStatus::Success);
        assert_eq!(
            console.text(),
            "running 0 tests\ntest result: ok. 0 passed\n"
        );
    }

    #[test]
    fn failing_test_name_is_printed_before_it_runs() {
        let mut console = RecordingConsole::new();
        let cases = [ktest_case!(passing), ktest_case!(failing)];

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(&mut console, &[&cases], None)
        }));

        assert!(result.is_err(), "a failing case must not report success");
        assert!(console.text().ends_with("test failing ... "));
    }
}
