//! QEMU `isa-debug-exit` device: lets the kernel set QEMU's exit status.
//!
//! Writing value `v` makes QEMU exit with status `(v << 1) | 1`:
//! `0x10` -> 33 (success), `0x11` -> 35 (failure). See design decision D6.

use super::port::outl;
use rhizo_hal::{ExitStatus, Power};

/// I/O port of the exit device (`-device isa-debug-exit,iobase=0xf4,iosize=0x04`).
const EXIT_PORT: u16 = 0xF4;

/// Power control through QEMU's debug exit device.
#[derive(Debug)]
pub struct QemuExit;

impl Power for QemuExit {
    fn exit(&mut self, status: ExitStatus) -> ! {
        let code = match status {
            ExitStatus::Success => 0x10,
            ExitStatus::Failure => 0x11,
        };
        // SAFETY: port 0xF4 is the QEMU debug exit device configured by
        // `xtask`; writing to it terminates the VM.
        unsafe { outl(EXIT_PORT, code) };
        // Without the device (e.g. another hypervisor) the write does nothing:
        // stop the CPU instead of returning.
        super::halt_forever()
    }
}
