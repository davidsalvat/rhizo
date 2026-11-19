//! Messages printed while the kernel boots.

use core::fmt::Write;
use rhizo_hal::{Console, ConsoleWriter};

/// Prints the boot banner identifying the project and kernel version.
pub fn print_banner(console: &mut impl Console) {
    // Writing to a `Console` cannot fail, so the `fmt::Result` is always Ok.
    let _ = writeln!(ConsoleWriter(console), "Rhizo {}", crate::VERSION);
}

/// Prints how long boot took and that boot has finished.
///
/// `start` and `end` are cycle counts taken at kernel entry and at the end of
/// boot. A counter that went backwards is reported as zero cycles rather than
/// wrapping around.
pub fn report_boot_complete(console: &mut impl Console, start: u64, end: u64) {
    let mut out = ConsoleWriter(console);
    let _ = writeln!(out, "boot time: {} cycles", end.saturating_sub(start));
    let _ = writeln!(out, "boot complete");
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhizo_hal::CycleCounter;
    use rhizo_hal::testing::{ManualCycleCounter, RecordingConsole};

    #[test]
    fn banner_starts_with_project_name_and_version() {
        let mut console = RecordingConsole::new();
        print_banner(&mut console);
        assert_eq!(console.text(), format!("Rhizo {}\n", crate::VERSION));
    }

    #[test]
    fn boot_report_shows_duration_and_completion() {
        let counter = ManualCycleCounter::new(1_000);
        let start = counter.cycles();
        counter.advance(2_500);

        let mut console = RecordingConsole::new();
        report_boot_complete(&mut console, start, counter.cycles());

        assert_eq!(console.text(), "boot time: 2500 cycles\nboot complete\n");
    }

    #[test]
    fn boot_report_never_wraps_on_backwards_counter() {
        let mut console = RecordingConsole::new();
        report_boot_complete(&mut console, 10, 5);
        assert!(console.text().starts_with("boot time: 0 cycles\n"));
    }
}
