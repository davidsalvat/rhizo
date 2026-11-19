//! Messages printed while the kernel boots.

use core::fmt::Write;
use rhizo_hal::{Console, ConsoleWriter};

/// Prints the boot banner identifying the project and kernel version.
pub fn print_banner(console: &mut impl Console) {
    // Writing to a `Console` cannot fail, so the `fmt::Result` is always Ok.
    let _ = writeln!(ConsoleWriter(console), "Rhizo {}", crate::VERSION);
}

/// Prints the machine-readable ready marker: `rhizo-ready cycles=<n>`.
///
/// The host timestamps the arrival of this line to measure time to ready.
pub fn report_ready(console: &mut impl Console, start: u64, end: u64) {
    let _ = writeln!(
        ConsoleWriter(console),
        "rhizo-ready cycles={}",
        end.saturating_sub(start)
    );
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

/// Prints the memory summary: usable RAM and RAM still free in MiB, and the
/// frames used by the kernel's page tables (a deterministic benchmark metric).
pub fn report_memory(
    console: &mut impl Console,
    usable_bytes: u64,
    free_bytes: u64,
    page_table_frames: u64,
) {
    const MIB: u64 = 1024 * 1024;
    let _ = writeln!(
        ConsoleWriter(console),
        "memory: {} MiB usable, {} MiB free, {} page-table frames",
        usable_bytes / MIB,
        free_bytes / MIB,
        page_table_frames
    );
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
    fn ready_marker_carries_cycles() {
        let mut console = RecordingConsole::new();
        report_ready(&mut console, 100, 350);
        assert_eq!(console.text(), "rhizo-ready cycles=250\n");
    }

    #[test]
    fn memory_report_is_in_mib() {
        let mut console = RecordingConsole::new();
        report_memory(&mut console, 127 * 1024 * 1024 + 5, 120 * 1024 * 1024, 7);
        assert_eq!(
            console.text(),
            "memory: 127 MiB usable, 120 MiB free, 7 page-table frames\n"
        );
    }

    #[test]
    fn boot_report_never_wraps_on_backwards_counter() {
        let mut console = RecordingConsole::new();
        report_boot_complete(&mut console, 10, 5);
        assert!(console.text().starts_with("boot time: 0 cycles\n"));
    }
}
