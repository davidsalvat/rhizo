//! Minimal 16550 UART driver (polling, transmit only).

use super::port::{inb, outb};
use rhizo_hal::Console;

/// I/O base of the first serial port.
const COM1: u16 = 0x3F8;

/// Line status register bit: transmit holding register empty.
const LSR_THR_EMPTY: u8 = 1 << 5;

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
