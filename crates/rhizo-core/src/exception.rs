//! CPU exception reports.

use core::fmt::{self, Write};
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
    write_exception(console, format_args!("\nEXCEPTION: "), info);
}

/// Prints the report of an exception raised by cell `cell`, which ends that
/// cell: the same details as [`report_exception`] after `cell N killed: `.
pub fn report_cell_exception(console: &mut impl Console, cell: u32, info: &ExceptionInfo) {
    write_exception(console, format_args!("cell {cell} killed: "), info);
}

fn write_exception(console: &mut impl Console, prefix: fmt::Arguments<'_>, info: &ExceptionInfo) {
    let mut out = ConsoleWriter(console);
    let _ = writeln!(
        out,
        "{prefix}{} (vector {}, error {:#x}) at rip {:#x}",
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

/// What the kernel knows about an AArch64 synchronous exception.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aarch64Exception {
    /// `ESR_EL1`: exception syndrome.
    pub esr: u64,
    /// `FAR_EL1`: faulting address (valid for aborts).
    pub far: u64,
    /// `ELR_EL1`: address of the faulting instruction.
    pub elr: u64,
    /// The kernel stack's guard page `[start, end)`, to recognize overflows.
    pub stack_guard: (u64, u64),
}

/// Name of an AArch64 exception class (`ESR_EL1.EC`).
pub fn aarch64_class_name(ec: u64) -> &'static str {
    match ec {
        0x00 => "undefined instruction",
        0x15 => "supervisor call",
        0x18 => "system register trap",
        0x20 | 0x21 => "instruction abort",
        0x22 => "PC alignment fault",
        0x24 | 0x25 => "data abort",
        0x26 => "SP alignment fault",
        0x3c => "breakpoint",
        _ => "exception",
    }
}

/// A read of an ID register by cell code (`MRS Xt, <reg>` with op0 3, op1 0,
/// CRn 0, CRm 0 to 7: `MIDR_EL1`, `ID_AA64ISAR0_EL1`...), which EL0 cannot
/// do (change `add-pip-packages`): the kernel answers it as Linux does
/// for programs told so by `HWCAP_CPUID`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdRegisterRead {
    /// The destination register (31: `XZR`, the value is dropped).
    pub rt: usize,
    /// `CRm` and `op2`: which register of the ID space.
    pub crm: u8,
    pub op2: u8,
}

impl IdRegisterRead {
    /// The read an EL0 exception stands for, if it is one: a trapped
    /// system register access (class 0x18, decoded from the syndrome) or an
    /// undefined instruction (class 0x0, decoded from the instruction
    /// `insn` returns, the word at the faulting address).
    pub fn decode(esr: u64, insn: impl FnOnce() -> Option<u32>) -> Option<Self> {
        // (op0, op1, CRn, CRm, op2, Rt, read)
        let (op0, op1, crn, crm, op2, rt, read) = match (esr >> 26) & 0x3f {
            0x18 => {
                let iss = esr as u32 & 0x1ff_ffff;
                let f = |shift: u32, bits: u32| (iss >> shift) & ((1 << bits) - 1);
                (
                    f(20, 2),
                    f(14, 3),
                    f(10, 4),
                    f(1, 4),
                    f(17, 3),
                    f(5, 5),
                    f(0, 1) == 1,
                )
            }
            0x00 => {
                let insn = insn()?;
                let f = |shift: u32, bits: u32| (insn >> shift) & ((1 << bits) - 1);
                // MRS: 1101_0101_0011 in bits 31-20 (L set, op0 2 or 3).
                (
                    f(19, 2),
                    f(16, 3),
                    f(12, 4),
                    f(8, 4),
                    f(5, 3),
                    f(0, 5),
                    insn >> 20 == 0xd53,
                )
            }
            _ => return None,
        };
        (read && op0 == 3 && op1 == 0 && crn == 0 && crm < 8).then_some(Self {
            rt: rt as usize,
            crm: crm as u8,
            op2: op2 as u8,
        })
    }

    /// The value cell code reads: the CPU's `MIDR_EL1` and `REVIDR_EL1`
    /// (`midr`, `revidr`) as they are, `MPIDR_EL1` with only bit 31 set (no
    /// CPU numbers: a cell may run on any), and zero for every other
    /// register (features are told by the auxiliary vector, not here).
    pub fn value(&self, midr: u64, revidr: u64) -> u64 {
        match (self.crm, self.op2) {
            (0, 0) => midr,
            (0, 5) => 1 << 31,
            (0, 6) => revidr,
            _ => 0,
        }
    }
}

/// Prints an AArch64 exception report, in the same shape as the x86_64 one:
///
/// ```text
/// EXCEPTION: data abort (class 0x25, ESR 0x96000045) at pc 0xffffffff80081234
///   address 0x0: write, translation fault (level 0), kernel mode
/// ```
pub fn report_aarch64(console: &mut impl Console, info: &Aarch64Exception) {
    write_aarch64(console, format_args!("\nEXCEPTION: "), info);
}

/// Prints the report of an AArch64 exception raised by cell `cell`, which
/// ends that cell: the same details as [`report_aarch64`] after
/// `cell N killed: `.
pub fn report_cell_aarch64(console: &mut impl Console, cell: u32, info: &Aarch64Exception) {
    write_aarch64(console, format_args!("cell {cell} killed: "), info);
}

fn write_aarch64(console: &mut impl Console, prefix: fmt::Arguments<'_>, info: &Aarch64Exception) {
    let mut out = ConsoleWriter(console);
    let ec = (info.esr >> 26) & 0x3f;
    let _ = writeln!(
        out,
        "{prefix}{} (class {:#x}, ESR {:#x}) at pc {:#x}",
        aarch64_class_name(ec),
        ec,
        info.esr,
        info.elr
    );
    if matches!(ec, 0x20 | 0x21 | 0x24 | 0x25) {
        let fsc = info.esr & 0x3f;
        let level = fsc & 0b11;
        let cause = match fsc >> 2 {
            0b0000 => "address size fault",
            0b0001 => "translation fault",
            0b0010 => "access flag fault",
            0b0011 => "permission fault",
            _ if fsc == 0b10_0001 => "alignment fault",
            _ => "other fault",
        };
        let access = if matches!(ec, 0x20 | 0x21) {
            "instruction fetch"
        } else if info.esr & (1 << 6) != 0 {
            "write"
        } else {
            "read"
        };
        let mode = if matches!(ec, 0x20 | 0x24) {
            "user"
        } else {
            "kernel"
        };
        let _ = writeln!(
            out,
            "  address {:#x}: {access}, {cause} (level {level}), {mode} mode",
            info.far
        );
        let (guard_start, guard_end) = info.stack_guard;
        if (guard_start..guard_end).contains(&info.far) {
            let _ = writeln!(out, "  kernel stack overflow (guard page hit)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_register_reads_are_decoded_from_the_instruction_or_the_syndrome() {
        let undefined = 0; // class 0x0
        let read = |insn: u32| IdRegisterRead::decode(undefined, || Some(insn));
        // mrs x3, midr_el1
        let midr = read(0xd538_0003).expect("an ID register read");
        assert_eq!((midr.rt, midr.value(0x410f_d0c1, 7)), (3, 0x410f_d0c1));
        // mrs x0, mpidr_el1; mrs x1, revidr_el1; mrs x2, id_aa64isar0_el1
        assert_eq!(read(0xd538_00a0).map(|r| r.value(1, 7)), Some(1 << 31));
        assert_eq!(read(0xd538_00c1).map(|r| r.value(1, 7)), Some(7));
        assert_eq!(
            read(0xd538_0602).map(|r| (r.rt, r.value(1, 7))),
            Some((2, 0))
        );
        // Not ID registers: mrs x0, ctr_el0 (op1 3); msr midr_el1, x0 (a
        // write); a plain instruction (nop); no instruction readable.
        for insn in [0xd53b_0020, 0xd518_0000, 0xd503_201f] {
            assert_eq!(read(insn), None, "{insn:#x}");
        }
        assert_eq!(IdRegisterRead::decode(undefined, || None), None);
        // Class 0x18 (FEAT_IDST): op0 3, Rt 5, read; the same as a write.
        let trapped = (0x18 << 26) | (3 << 20) | (5 << 5);
        let r = IdRegisterRead::decode(trapped | 1, || panic!("not read"));
        assert_eq!(r.map(|r| (r.rt, r.value(9, 0))), Some((5, 9)));
        assert_eq!(IdRegisterRead::decode(trapped, || None), None);
        // Any other exception, such as a data abort, is none.
        assert_eq!(
            IdRegisterRead::decode(0x9200_0005, || panic!("not read")),
            None
        );
    }
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
    fn aarch64_data_abort_report() {
        let mut console = RecordingConsole::new();
        // EC 0x25 (data abort, same EL), WnR = 1, DFSC = translation fault level 0.
        let esr = (0x25 << 26) | (1 << 25) | (1 << 6) | 0b000100;
        report_aarch64(
            &mut console,
            &Aarch64Exception {
                esr,
                far: 0,
                elr: 0x1000,
                stack_guard: (0x8000, 0x9000),
            },
        );
        assert_eq!(
            console.text(),
            "\nEXCEPTION: data abort (class 0x25, ESR 0x96000044) at pc 0x1000\n  \
             address 0x0: write, translation fault (level 0), kernel mode\n"
        );
    }

    #[test]
    fn cell_faults_carry_the_cell_and_user_mode() {
        let mut console = RecordingConsole::new();
        // EC 0x24 (data abort from EL0), write, permission fault level 3.
        let esr = (0x24 << 26) | (1 << 25) | (1 << 6) | 0b001111;
        let info = Aarch64Exception {
            esr,
            far: 0xffff_8000_0000_0000,
            elr: 0x40_0010,
            stack_guard: (0, 0),
        };
        report_cell_aarch64(&mut console, 4, &info);
        assert_eq!(
            console.text(),
            "cell 4 killed: data abort (class 0x24, ESR 0x9200004f) at pc 0x400010\n  \
             address 0xffff800000000000: write, permission fault (level 3), user mode\n"
        );
        let mut console = RecordingConsole::new();
        let info = ExceptionInfo {
            vector: 14,
            error_code: 0x6,
            rip: 0x40_0010,
            fault_address: Some(0),
        };
        report_cell_exception(&mut console, 9, &info);
        assert_eq!(
            console.text(),
            "cell 9 killed: page fault (vector 14, error 0x6) at rip 0x400010\n  \
             address 0x0: write, page not present, user mode\n"
        );
    }

    #[test]
    fn aarch64_stack_overflow_is_recognized() {
        let mut console = RecordingConsole::new();
        let esr = (0x25 << 26) | (1 << 6) | 0b000111; // write, translation fault level 3
        report_aarch64(
            &mut console,
            &Aarch64Exception {
                esr,
                far: 0x8ff0,
                elr: 0x2000,
                stack_guard: (0x8000, 0x9000),
            },
        );
        assert!(console.text().ends_with(
            "  address 0x8ff0: write, translation fault (level 3), kernel mode\n  \
             kernel stack overflow (guard page hit)\n"
        ));
    }

    #[test]
    fn aarch64_other_classes() {
        let report = |esr: u64| {
            let mut console = RecordingConsole::new();
            report_aarch64(
                &mut console,
                &Aarch64Exception {
                    esr,
                    far: 0x40,
                    elr: 0,
                    stack_guard: (0, 0),
                },
            );
            console.text()
        };
        assert_eq!(
            report(0x0200_0000),
            "\nEXCEPTION: undefined instruction (class 0x0, ESR 0x2000000) at pc 0x0\n"
        );
        assert!(
            report((0x21 << 26) | 0b001111)
                .contains("instruction fetch, permission fault (level 3), kernel mode")
        );
        assert!(
            report((0x24 << 26) | 0b000101)
                .contains("read, translation fault (level 1), user mode")
        );
        assert_eq!(aarch64_class_name(0x3c), "breakpoint");
        assert_eq!(aarch64_class_name(0x3f), "exception");
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
