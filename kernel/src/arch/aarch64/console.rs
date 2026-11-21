//! The boot console: the UART the device tree describes (PL011 or 16550),
//! polled transmit, reached through the device-memory window.

use super::MMIO_BASE;
use super::platform::{self, Uart};
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use rhizo_core::options::ConsoleKind;
use rhizo_hal::Console;

const NONE: u8 = 0;
const PL011: u8 = 1;
const NS16550: u8 = 2;

static KIND: AtomicU8 = AtomicU8::new(NONE);
static BASE: AtomicU64 = AtomicU64::new(0);

/// Finds the UART in the device tree. AArch64 VMMs offer no debug port, so
/// the requested kind does not matter.
pub fn init(_kind: ConsoleKind) {
    let (kind, region) = match platform::uart() {
        Some(Uart::Pl011(r)) => (PL011, r),
        Some(Uart::Ns16550(r)) => (NS16550, r),
        None => return,
    };
    BASE.store(MMIO_BASE + region.base, Ordering::Relaxed);
    KIND.store(kind, Ordering::Relaxed);
    #[cfg(feature = "control")]
    if kind == PL011 {
        const CR_UARTEN_TXE_RXE: u32 = (1 << 0) | (1 << 8) | (1 << 9);
        let cr = (MMIO_BASE + region.base + 0x30) as *mut u32;
        // SAFETY: CR (offset 0x30) of the PL011 the device tree describes,
        // mapped as device memory: enabling the UART and its receiver only
        // lets it take bytes from the host.
        unsafe { cr.write_volatile(CR_UARTEN_TXE_RXE) };
    }
}

/// The control channel (change `add-control-plane`): the UART, both ways,
/// shared with the console's text.
#[cfg(feature = "control")]
pub mod control {
    use super::{BASE, KIND, NS16550, Ordering, PL011, write_byte};

    /// The next byte the host sent, if one is waiting.
    pub fn read_byte() -> Option<u8> {
        let base = BASE.load(Ordering::Relaxed);
        match KIND.load(Ordering::Relaxed) {
            PL011 => {
                const FR_RXFE: u32 = 1 << 4;
                let dr = base as *const u32;
                let fr = (base + 0x18) as *const u32;
                // SAFETY: DR and FR of the PL011 found at boot, mapped as
                // device memory; reading DR only takes the received byte.
                unsafe { (fr.read_volatile() & FR_RXFE == 0).then(|| dr.read_volatile() as u8) }
            }
            NS16550 => {
                const LSR_DR: u8 = 1 << 0;
                let rbr = base as *const u8;
                let lsr = (base + 5) as *const u8;
                // SAFETY: as above, for the 16550's RBR and LSR.
                unsafe { (lsr.read_volatile() & LSR_DR != 0).then(|| rbr.read_volatile()) }
            }
            _ => None,
        }
    }

    /// Sends `bytes` (a frame) to the host as they are.
    pub fn write(bytes: &[u8]) {
        let kind = KIND.load(Ordering::Relaxed);
        let base = BASE.load(Ordering::Relaxed);
        for &byte in bytes {
            write_byte(kind, base, byte);
        }
    }
}

/// Handle to the boot console (stateless).
#[derive(Debug, Clone, Copy)]
pub struct BootConsole;

fn write_byte(kind: u8, base: u64, byte: u8) {
    match kind {
        PL011 => {
            const FR_TXFF: u32 = 1 << 5;
            let dr = base as *mut u32;
            let fr = (base + 0x18) as *const u32;
            // SAFETY: `base` is the PL011 the device tree describes, mapped as
            // device memory; DR (offset 0) and FR (0x18) are its registers.
            unsafe {
                while fr.read_volatile() & FR_TXFF != 0 {
                    core::hint::spin_loop();
                }
                dr.write_volatile(byte as u32);
            }
        }
        NS16550 => {
            const LSR_THRE: u8 = 1 << 5;
            let thr = base as *mut u8;
            let lsr = (base + 5) as *const u8;
            // SAFETY: as above, for a 16550 with 8-bit registers.
            unsafe {
                while lsr.read_volatile() & LSR_THRE == 0 {
                    core::hint::spin_loop();
                }
                thr.write_volatile(byte);
            }
        }
        _ => {}
    }
}

impl Console for BootConsole {
    fn write_bytes(&mut self, bytes: &[u8]) {
        let kind = KIND.load(Ordering::Relaxed);
        let base = BASE.load(Ordering::Relaxed);
        for &byte in bytes {
            if byte == b'\n' {
                write_byte(kind, base, b'\r');
            }
            write_byte(kind, base, byte);
        }
    }
}
