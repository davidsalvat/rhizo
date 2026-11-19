//! Boot options from the kernel command line.
//!
//! The VMM passes a command line through the PVH start info. Recognized
//! options (unknown ones are ignored):
//!
//! - `console=e9` (debug port 0xE9) or `console=serial` (default)
//! - `exit=debug-exit` (QEMU `isa-debug-exit`, default), `exit=i8042`
//!   (keyboard-controller reset, Firecracker) or `exit=ch-acpi`
//!   (Cloud Hypervisor ACPI shutdown device), `exit=psci` (AArch64 PSCI) or
//!   `exit=semihosting` (AArch64 under QEMU emulation)

use rhizo_hal::PhysicalMemory;

/// Where boot output goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleKind {
    /// First 16550 serial port.
    Serial,
    /// Debug port 0xE9 (QEMU `-debugcon`, Cloud Hypervisor `--debug-console`).
    DebugPort,
}

/// How the kernel ends the virtual machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    /// QEMU `isa-debug-exit`: carries a success/failure status.
    DebugExit,
    /// Keyboard-controller reset; the VMM process ends.
    I8042Reset,
    /// Cloud Hypervisor ACPI shutdown device; the VMM process ends.
    CloudHypervisorAcpi,
    /// PSCI `SYSTEM_OFF` (AArch64); the VMM process ends.
    Psci,
    /// Semihosting `SYS_EXIT_EXTENDED` (AArch64 under QEMU emulation):
    /// carries a success/failure status.
    Semihosting,
}

/// Parsed boot options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootOptions {
    /// Boot console.
    pub console: ConsoleKind,
    /// Exit mechanism.
    pub exit: ExitKind,
}

impl Default for BootOptions {
    fn default() -> Self {
        Self {
            console: ConsoleKind::Serial,
            exit: ExitKind::DebugExit,
        }
    }
}

impl BootOptions {
    /// Parses a command line; unknown or malformed options keep defaults.
    pub fn parse(cmdline: &[u8]) -> Self {
        let mut options = Self::default();
        for word in cmdline.split(|b| b.is_ascii_whitespace()) {
            match word {
                b"console=e9" => options.console = ConsoleKind::DebugPort,
                b"console=serial" => options.console = ConsoleKind::Serial,
                b"exit=debug-exit" => options.exit = ExitKind::DebugExit,
                b"exit=i8042" => options.exit = ExitKind::I8042Reset,
                b"exit=ch-acpi" => options.exit = ExitKind::CloudHypervisorAcpi,
                b"exit=psci" => options.exit = ExitKind::Psci,
                b"exit=semihosting" => options.exit = ExitKind::Semihosting,
                _ => {}
            }
        }
        options
    }
}

/// Offset of `cmdline_paddr` in `hvm_start_info`.
const CMDLINE_OFFSET: u64 = 24;

/// Copies the NUL-terminated command line referenced by `hvm_start_info` at
/// `start_info` into `buf` and returns the copied bytes (without the NUL).
/// Returns an empty slice when there is no command line.
pub fn read_cmdline<'b>(mem: &impl PhysicalMemory, start_info: u64, buf: &'b mut [u8]) -> &'b [u8] {
    let addr = mem.read_u64(start_info + CMDLINE_OFFSET);
    if addr == 0 {
        return &[];
    }
    let mut len = 0;
    while len < buf.len() {
        let at = addr + len as u64;
        let byte = (mem.read_u64(at & !7) >> ((at & 7) * 8)) as u8;
        if byte == 0 {
            break;
        }
        buf[len] = byte;
        len += 1;
    }
    &buf[..len]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhizo_hal::testing::FakePhysicalMemory;

    #[test]
    fn defaults_when_empty_or_unknown() {
        assert_eq!(BootOptions::parse(b""), BootOptions::default());
        assert_eq!(
            BootOptions::parse(b"quiet console=ttyS0 exit=bogus"),
            BootOptions::default()
        );
    }

    #[test]
    fn recognizes_every_option() {
        let o = BootOptions::parse(b"console=e9  exit=i8042");
        assert_eq!(o.console, ConsoleKind::DebugPort);
        assert_eq!(o.exit, ExitKind::I8042Reset);
        assert_eq!(
            BootOptions::parse(b"exit=ch-acpi").exit,
            ExitKind::CloudHypervisorAcpi
        );
        assert_eq!(BootOptions::parse(b"exit=psci").exit, ExitKind::Psci);
        assert_eq!(
            BootOptions::parse(b"exit=semihosting").exit,
            ExitKind::Semihosting
        );
        assert_eq!(
            BootOptions::parse(b"console=e9 console=serial").console,
            ConsoleKind::Serial
        );
        assert_eq!(
            BootOptions::parse(b"exit=i8042 exit=debug-exit").exit,
            ExitKind::DebugExit
        );
    }

    fn write_str(mem: &mut FakePhysicalMemory, addr: u64, bytes: &[u8]) {
        for (i, chunk) in bytes.chunks(1).enumerate() {
            let at = addr + i as u64;
            let word = mem.read_u64(at & !7);
            let shift = (at & 7) * 8;
            mem.write_u64(
                at & !7,
                (word & !(0xff << shift)) | ((chunk[0] as u64) << shift),
            );
        }
    }

    #[test]
    fn reads_unaligned_command_line() {
        let mut mem = FakePhysicalMemory::new();
        mem.write_u64(0x6000 + CMDLINE_OFFSET, 0x7003);
        write_str(&mut mem, 0x7003, b"console=e9 exit=i8042\0junk");
        let mut buf = [0u8; 64];
        assert_eq!(
            read_cmdline(&mem, 0x6000, &mut buf),
            b"console=e9 exit=i8042"
        );
    }

    #[test]
    fn missing_command_line_is_empty_and_long_ones_are_truncated() {
        let mut mem = FakePhysicalMemory::new();
        let mut buf = [0u8; 4];
        assert_eq!(read_cmdline(&mem, 0x6000, &mut buf), b"");
        mem.write_u64(0x6000 + CMDLINE_OFFSET, 0x7000);
        write_str(&mut mem, 0x7000, b"abcdefgh\0");
        assert_eq!(read_cmdline(&mem, 0x6000, &mut buf), b"abcd");
    }
}
