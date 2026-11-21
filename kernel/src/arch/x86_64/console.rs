//! The boot console: debug port 0xE9 or the first serial port, chosen once at
//! boot from the command line (design decision D2).

use super::port::outb;
use super::serial::Serial;
use core::sync::atomic::{AtomicBool, Ordering};
use rhizo_core::options::ConsoleKind;
use rhizo_hal::Console;

/// Debug console port (QEMU `-debugcon`, Cloud Hypervisor `--debug-console`).
const DEBUG_PORT: u16 = 0xE9;

static USE_DEBUG_PORT: AtomicBool = AtomicBool::new(false);

/// Selects and initializes the boot console. Call once, first thing in boot.
pub fn init(kind: ConsoleKind) {
    match kind {
        ConsoleKind::DebugPort => USE_DEBUG_PORT.store(true, Ordering::Relaxed),
        ConsoleKind::Serial => Serial::com1().init(),
    }
}

/// Handle to the boot console. Stateless, so any number can exist (the kernel
/// runs on one CPU with interrupts disabled).
#[derive(Debug, Clone, Copy)]
pub struct BootConsole;

/// The control channel (change `add-control-plane`): the serial port, both
/// ways, shared with the console's text; it needs `console=serial`.
#[cfg(feature = "control")]
pub mod control {
    use super::{Ordering, Serial, USE_DEBUG_PORT};

    /// The next byte the host sent, if one is waiting.
    pub fn read_byte() -> Option<u8> {
        if USE_DEBUG_PORT.load(Ordering::Relaxed) {
            return None;
        }
        Serial::com1().read_byte()
    }

    /// Sends `bytes` (a frame) to the host as they are.
    pub fn write(bytes: &[u8]) {
        Serial::com1().write_raw(bytes);
    }
}

impl Console for BootConsole {
    fn write_bytes(&mut self, bytes: &[u8]) {
        if USE_DEBUG_PORT.load(Ordering::Relaxed) {
            for &byte in bytes {
                // SAFETY: port 0xE9 is the debug console selected by the VMM's
                // command line; writes only emit the byte.
                unsafe { outb(DEBUG_PORT, byte) };
            }
        } else {
            Serial::com1().write_bytes(bytes);
        }
    }
}
