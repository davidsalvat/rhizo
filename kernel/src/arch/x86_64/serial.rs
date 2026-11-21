//! Minimal 16550 UART driver (polling; receiving in control builds only).

use super::port::{inb, outb};
#[cfg(feature = "control")]
use core::sync::atomic::{AtomicBool, Ordering};
use rhizo_hal::Console;

/// I/O base of the first serial port.
const COM1: u16 = 0x3F8;

/// Line status register bit: transmit holding register empty.
const LSR_THR_EMPTY: u8 = 1 << 5;
/// Line status register bit: a received byte is waiting.
#[cfg(feature = "control")]
const LSR_DATA_READY: u8 = 1 << 0;
/// Bytes the transmit FIFO takes once the line status says it is empty.
#[cfg(feature = "control")]
const TX_FIFO: usize = 16;

/// Whether [`Serial::init`] turned the FIFOs on. Each register access traps
/// to the VMM, so control frames fill the transmit FIFO after one look at
/// the line status instead of looking before every byte (change
/// `measure-control-path`).
#[cfg(feature = "control")]
static FIFOS_ON: AtomicBool = AtomicBool::new(false);

/// A 16550-compatible serial port.
#[derive(Debug)]
pub struct Serial {
    base: u16,
}

impl Serial {
    /// The first serial port (COM1).
    pub const fn com1() -> Self {
        Self { base: COM1 }
    }

    /// Configures the port for 115200 baud, 8N1, FIFOs on, interrupts off.
    pub fn init(&mut self) {
        let b = self.base;
        // SAFETY: COM1 is a standard 16550 UART on the platforms we support
        // (QEMU q35/pc); this is its documented initialization sequence.
        unsafe {
            outb(b + 1, 0x00); // disable interrupts
            outb(b + 3, 0x80); // enable divisor latch
            outb(b, 0x01); // divisor low byte: 115200 baud
            outb(b + 1, 0x00); // divisor high byte
            outb(b + 3, 0x03); // 8 bits, no parity, one stop bit
            outb(b + 2, 0xC7); // enable and clear FIFOs, 14-byte threshold
            outb(b + 4, 0x03); // DTR | RTS
        }
        #[cfg(feature = "control")]
        FIFOS_ON.store(true, Ordering::Relaxed);
    }

    /// The next received byte, if one is waiting.
    #[cfg(feature = "control")]
    pub fn read_byte(&mut self) -> Option<u8> {
        // SAFETY: reading the line status register has no side effect, and
        // reading the receive buffer of COM1 only takes the byte it holds.
        unsafe { (inb(self.base + 5) & LSR_DATA_READY != 0).then(|| inb(self.base)) }
    }

    /// Sends `bytes` as they are (no `\r` before `\n`).
    #[cfg(feature = "control")]
    pub fn write_raw(&mut self, bytes: &[u8]) {
        if !FIFOS_ON.load(Ordering::Relaxed) {
            bytes.iter().for_each(|&byte| self.write_byte(byte));
            return;
        }
        for burst in bytes.chunks(TX_FIFO) {
            // SAFETY: reading the line status register and writing the
            // transmit register of COM1 have no side effects beyond sending
            // the bytes; once the transmit FIFO is empty it takes `TX_FIFO`.
            unsafe {
                while inb(self.base + 5) & LSR_THR_EMPTY == 0 {
                    core::hint::spin_loop();
                }
                for &byte in burst {
                    outb(self.base, byte);
                }
            }
        }
    }

    fn write_byte(&mut self, byte: u8) {
        // SAFETY: reading the line status register and writing the transmit
        // register of COM1 have no side effects beyond sending the byte.
        unsafe {
            while inb(self.base + 5) & LSR_THR_EMPTY == 0 {
                core::hint::spin_loop();
            }
            outb(self.base, byte);
        }
    }
}

impl Console for Serial {
    fn write_bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
    }
}
