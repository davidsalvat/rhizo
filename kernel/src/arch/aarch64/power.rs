//! Ending the virtual machine: PSCI `SYSTEM_OFF`, or semihosting
//! `SYS_EXIT_EXTENDED` under QEMU emulation, which carries the status.

use core::sync::atomic::{AtomicU8, Ordering};
use rhizo_core::options::ExitKind;
use rhizo_hal::{ExitStatus, Power};

const PSCI_SYSTEM_OFF: u64 = 0x8400_0008;
const SYS_EXIT_EXTENDED: u64 = 0x20;
const ADP_STOPPED_APPLICATION_EXIT: u64 = 0x2_0026;
/// Same process statuses as QEMU's x86 exit device, so `xtask` treats both
/// alike: 33 success, 35 failure.
const STATUS_SUCCESS: u64 = 33;
const STATUS_FAILURE: u64 = 35;

static SEMIHOSTING: AtomicU8 = AtomicU8::new(0);

/// Selects how the VM is ended. Call once at boot.
pub fn init(kind: ExitKind) {
    SEMIHOSTING.store(u8::from(kind == ExitKind::Semihosting), Ordering::Relaxed);
}

/// Power control for the platform selected at boot.
#[derive(Debug)]
pub struct Machine;

impl Power for Machine {
    fn exit(&mut self, status: ExitStatus) -> ! {
        if SEMIHOSTING.load(Ordering::Relaxed) == 1 {
            let code = match status {
                ExitStatus::Success => STATUS_SUCCESS,
                ExitStatus::Failure => STATUS_FAILURE,
            };
            let block = [ADP_STOPPED_APPLICATION_EXIT, code];
            // SAFETY: semihosting call selected on the command line by the
            // host (QEMU -semihosting); `block` is a valid two-word parameter
            // block. The call ends the VM.
            unsafe {
                core::arch::asm!("hlt #0xf000", in("x0") SYS_EXIT_EXTENDED, in("x1") block.as_ptr());
            }
        }
        let hvc = super::platform::device_tree().psci_method().ok().flatten() != Some(b"smc");
        // SAFETY: PSCI SYSTEM_OFF through the conduit the device tree names;
        // it does not return when the VMM implements it.
        unsafe {
            if hvc {
                core::arch::asm!("hvc #0", in("x0") PSCI_SYSTEM_OFF, options(nomem, nostack));
            } else {
                core::arch::asm!("smc #0", in("x0") PSCI_SYSTEM_OFF, options(nomem, nostack));
            }
        }
        super::halt_forever()
    }
}
