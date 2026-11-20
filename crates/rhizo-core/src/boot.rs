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

/// Prints how long boot took and that boot has finished. `cycles` is the
/// cycle count from kernel entry to the end of boot; `nanos` is the same
/// duration in nanoseconds when the architecture knows its counter frequency.
pub fn report_boot_complete(console: &mut impl Console, cycles: u64, nanos: Option<u64>) {
    let mut out = ConsoleWriter(console);
    let _ = match nanos {
        Some(ns) => writeln!(out, "boot time: {cycles} cycles ({} us)", ns / 1000),
        None => writeln!(out, "boot time: {cycles} cycles"),
    };
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

/// Prints how many CPUs are online: `cpus: <n> online`.
pub fn report_cpus(console: &mut impl Console, online: usize) {
    let _ = writeln!(ConsoleWriter(console), "cpus: {online} online");
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
        report_boot_complete(&mut console, counter.cycles() - start, None);

        assert_eq!(console.text(), "boot time: 2500 cycles\nboot complete\n");
    }

    #[test]
    fn boot_report_adds_microseconds_when_known() {
        let mut console = RecordingConsole::new();
        report_boot_complete(&mut console, 62_500, Some(1_250_400));
        assert_eq!(
            console.text(),
            "boot time: 62500 cycles (1250 us)\nboot complete\n"
        );
    }

    #[test]
    fn ready_marker_carries_cycles() {
        let mut console = RecordingConsole::new();
        report_ready(&mut console, 100, 350);
        assert_eq!(console.text(), "rhizo-ready cycles=250\n");
    }

    #[test]
    fn cpu_report() {
        let mut console = RecordingConsole::new();
        report_cpus(&mut console, 4);
        assert_eq!(console.text(), "cpus: 4 online\n");
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
}
