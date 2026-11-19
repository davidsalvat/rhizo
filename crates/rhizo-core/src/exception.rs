//! CPU exception reports.

use core::fmt::Write;
use rhizo_hal::{Console, ConsoleWriter};

/// What the kernel knows about an exception when it reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExceptionInfo {
    /// Exception vector (0–31).
    pub vector: u8,
    /// Error code pushed by the CPU, or 0 for exceptions without one.
    pub error_code: u64,
    /// Address of the faulting instruction.
    pub rip: u64,
    /// Faulting address (`CR2`), only for page faults.
    pub fault_address: Option<u64>,
}

/// Vector of the page-fault exception.
pub const PAGE_FAULT: u8 = 14;
/// Vector of the double-fault exception.
pub const DOUBLE_FAULT: u8 = 8;

/// Human-readable name of an exception vector.
pub fn name(vector: u8) -> &'static str {
    match vector {
        0 => "divide error",
        1 => "debug",
        2 => "non-maskable interrupt",
        3 => "breakpoint",
        4 => "overflow",
        5 => "bound range exceeded",
        6 => "invalid opcode",
        7 => "device not available",
        8 => "double fault",
        9 => "coprocessor segment overrun",
        10 => "invalid TSS",
        11 => "segment not present",
        12 => "stack-segment fault",
        13 => "general protection fault",
        14 => "page fault",
        16 => "x87 floating-point error",
        17 => "alignment check",
        18 => "machine check",
        19 => "SIMD floating-point error",
        20 => "virtualization exception",
        21 => "control protection exception",
        28 => "hypervisor injection exception",
        29 => "VMM communication exception",
        30 => "security exception",
        _ => "reserved exception",
    }
}

/// Prints an exception report.
///
/// ```text
/// EXCEPTION: page fault (vector 14, error 0x2) at rip 0xffffffff80101234
///   address 0x0: write, page not present, kernel mode
/// ```
pub fn report_exception(console: &mut impl Console, info: &ExceptionInfo) {
    let mut out = ConsoleWriter(console);
    let _ = writeln!(
        out,
        "\nEXCEPTION: {} (vector {}, error {:#x}) at rip {:#x}",
        name(info.vector),
        info.vector,
        info.error_code,
        info.rip
    );
    if info.vector == PAGE_FAULT {
        let code = info.error_code;
        let access = if code & (1 << 4) != 0 {
            "instruction fetch"
        } else if code & (1 << 1) != 0 {
            "write"
        } else {
            "read"
        };
        let cause = if code & 1 != 0 {
            "protection violation"
        } else {
            "page not present"
        };
        let mode = if code & (1 << 2) != 0 {
            "user"
        } else {
            "kernel"
        };
        let _ = writeln!(
            out,
            "  address {:#x}: {access}, {cause}, {mode} mode",
            info.fault_address.unwrap_or(0)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhizo_hal::testing::RecordingConsole;

    #[test]
    fn page_fault_report_decodes_the_error_code() {
        let mut console = RecordingConsole::new();
        report_exception(
            &mut console,
            &ExceptionInfo {
                vector: 14,
                error_code: 0x2,
                rip: 0x1234,
                fault_address: Some(0),
            },
        );
        assert_eq!(
            console.text(),
            "\nEXCEPTION: page fault (vector 14, error 0x2) at rip 0x1234\n  \
             address 0x0: write, page not present, kernel mode\n"
        );
    }

    #[test]
    fn page_fault_variants() {
        let line = |code| {
            let mut console = RecordingConsole::new();
            report_exception(
                &mut console,
                &ExceptionInfo {
                    vector: 14,
                    error_code: code,
                    rip: 0,
                    fault_address: Some(0x1000),
                },
            );
            console.text().lines().nth(2).unwrap().to_string()
        };
        assert_eq!(
            line(0x0),
            "  address 0x1000: read, page not present, kernel mode"
        );
        assert_eq!(
            line(0x3),
            "  address 0x1000: write, protection violation, kernel mode"
        );
        assert_eq!(
            line(0x11),
            "  address 0x1000: instruction fetch, protection violation, kernel mode"
        );
        assert_eq!(
            line(0x4),
            "  address 0x1000: read, page not present, user mode"
        );
    }

    #[test]
    fn other_exceptions_have_one_line() {
        let mut console = RecordingConsole::new();
        report_exception(
            &mut console,
            &ExceptionInfo {
                vector: 6,
                error_code: 0,
                rip: 0xffff_ffff_8010_0000,
                fault_address: None,
            },
        );
        assert_eq!(
            console.text(),
            "\nEXCEPTION: invalid opcode (vector 6, error 0x0) at rip 0xffffffff80100000\n"
        );
    }

    #[test]
    fn architectural_vectors_have_their_names() {
        let expected = [
            (0, "divide error"),
            (1, "debug"),
            (2, "non-maskable interrupt"),
            (3, "breakpoint"),
            (4, "overflow"),
            (5, "bound range exceeded"),
            (6, "invalid opcode"),
            (7, "device not available"),
            (8, "double fault"),
            (9, "coprocessor segment overrun"),
            (10, "invalid TSS"),
            (11, "segment not present"),
            (12, "stack-segment fault"),
            (13, "general protection fault"),
            (14, "page fault"),
            (16, "x87 floating-point error"),
            (17, "alignment check"),
            (18, "machine check"),
            (19, "SIMD floating-point error"),
            (20, "virtualization exception"),
            (21, "control protection exception"),
            (28, "hypervisor injection exception"),
            (29, "VMM communication exception"),
            (30, "security exception"),
        ];
        for (vector, text) in expected {
            assert_eq!(name(vector), text, "vector {vector}");
        }
    }

    #[test]
    fn every_vector_has_a_name() {
        assert_eq!(name(DOUBLE_FAULT), "double fault");
        assert_eq!(name(15), "reserved exception");
        for v in 0..32 {
            assert!(!name(v).is_empty());
        }
    }
}
