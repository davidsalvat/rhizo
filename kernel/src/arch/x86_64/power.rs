//! Ending the virtual machine (design decision D3).
//!
//! - QEMU `isa-debug-exit`: writing `v` makes QEMU exit with status
//!   `(v << 1) | 1`: `0x10` -> 33 (success), `0x11` -> 35 (failure).
//! - Firecracker: keyboard-controller reset (`0xFE` to port `0x64`) ends the
//!   VMM process; success or failure is read from the console by the host.
//! - Cloud Hypervisor: its ACPI shutdown device at port `0x600` (S5 sleep with
//!   the enable bit, value `0x34`) ends the VMM process.

use super::port::{outb, outl};
use core::sync::atomic::{AtomicU8, Ordering};
use rhizo_core::options::ExitKind;
use rhizo_hal::{ExitStatus, Power};

const DEBUG_EXIT_PORT: u16 = 0xF4;
const I8042_COMMAND_PORT: u16 = 0x64;
const I8042_RESET: u8 = 0xFE;
const CH_SHUTDOWN_PORT: u16 = 0x600;
const CH_S5_SLEEP: u8 = (5 << 2) | (1 << 5);

static EXIT: AtomicU8 = AtomicU8::new(0);

/// Selects how the VM is ended. Call once at boot.
pub fn init(kind: ExitKind) {
    EXIT.store(kind as u8, Ordering::Relaxed);
}

/// Power control for the platform selected at boot.
#[derive(Debug)]
pub struct Machine;

impl Power for Machine {
    fn exit(&mut self, status: ExitStatus) -> ! {
        let kind = EXIT.load(Ordering::Relaxed);
        // SAFETY: each port belongs to the exit device of the VMM named on the
        // command line; the write ends the VM. On a VMM without that device the
        // write is ignored and the CPU is halted below.
        unsafe {
            if kind == ExitKind::I8042Reset as u8 {
                outb(I8042_COMMAND_PORT, I8042_RESET);
            } else if kind == ExitKind::CloudHypervisorAcpi as u8 {
                outb(CH_SHUTDOWN_PORT, CH_S5_SLEEP);
            } else {
                let code = match status {
                    ExitStatus::Success => 0x10,
                    ExitStatus::Failure => 0x11,
                };
                outl(DEBUG_EXIT_PORT, code);
            }
        }
        super::halt_forever()
    }
}
