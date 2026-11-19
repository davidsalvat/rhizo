//! x86 port I/O.

/// Writes a byte to an I/O port.
///
/// # Safety
/// Writing to an arbitrary port can reconfigure hardware; the caller must
/// know which device owns `port` and that the write is valid for it.
pub unsafe fn outb(port: u16, value: u8) {
    // SAFETY: the caller guarantees the port write is valid.
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") value,
            options(nomem, nostack, preserves_flags));
    }
}

/// Writes a 32-bit value to an I/O port.
///
/// # Safety
/// Same as [`outb`].
pub unsafe fn outl(port: u16, value: u32) {
    // SAFETY: the caller guarantees the port write is valid.
    unsafe {
        core::arch::asm!("out dx, eax", in("dx") port, in("eax") value,
            options(nomem, nostack, preserves_flags));
    }
}

/// Reads a byte from an I/O port.
///
/// # Safety
/// Reading some ports has side effects; the caller must know the device.
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: the caller guarantees the port read is valid.
    unsafe {
        core::arch::asm!("in al, dx", in("dx") port, out("al") value,
            options(nomem, nostack, preserves_flags));
    }
    value
}
