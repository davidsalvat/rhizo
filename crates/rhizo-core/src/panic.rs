//! Formatting of kernel panic reports.

use core::fmt::{self, Write};
use rhizo_hal::{Console, ConsoleWriter};

/// Where in the source code a panic happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceLocation<'a> {
    /// Source file path.
    pub file: &'a str,
    /// Line number (1-based).
    pub line: u32,
    /// Column number (1-based).
    pub column: u32,
}

/// Prints a panic report: the message and, when known, its source location.
///
/// Output format: `PANIC at <file>:<line>:<column>: <message>`.
pub fn report_panic(
    console: &mut impl Console,
    message: impl fmt::Display,
    location: Option<SourceLocation<'_>>,
) {
    let mut out = ConsoleWriter(console);
    let _ = match location {
        Some(loc) => writeln!(
            out,
            "\nPANIC at {}:{}:{}: {}",
            loc.file, loc.line, loc.column, message
        ),
        None => writeln!(out, "\nPANIC at <unknown location>: {message}"),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhizo_hal::testing::RecordingConsole;

    #[test]
    fn report_includes_message_file_and_line() {
        let mut console = RecordingConsole::new();
        let location = SourceLocation {
            file: "kernel/src/main.rs",
            line: 42,
            column: 7,
        };
        report_panic(&mut console, "boom", Some(location));
        assert_eq!(console.text(), "\nPANIC at kernel/src/main.rs:42:7: boom\n");
    }

    #[test]
    fn report_without_location_still_shows_message() {
        let mut console = RecordingConsole::new();
        report_panic(&mut console, format_args!("code {}", 3), None);
        assert_eq!(console.text(), "\nPANIC at <unknown location>: code 3\n");
    }
}
