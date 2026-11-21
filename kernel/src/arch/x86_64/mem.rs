//! Page-sized zero and copy, and CRC-32C, with the instructions made for
//! them (change `fast-mem-primitives`): `rep stosq` and `rep movsq` (fast
//! strings on every x86_64; general registers only), and the SSE4.2
//! `crc32` instruction, which works on general registers too.

use core::arch::asm;
#[cfg(feature = "cells")]
use core::sync::atomic::{AtomicU8, Ordering};

/// Quadwords in a page, the unit of both primitives.
const PAGE_QWORDS: usize = 4096 / 8;

/// Fills the page at `dst` with zeros.
///
/// # Safety
/// `dst` is 4 KiB-aligned writable memory owned by the caller for the call.
#[inline]
pub unsafe fn zero_page(dst: *mut u8) {
    // SAFETY: guaranteed by the caller; the direction flag is clear in the
    // kernel (the System V ABI, and every entry clears it).
    unsafe {
        asm!(
            "rep stosq",
            inout("rcx") PAGE_QWORDS => _,
            inout("rdi") dst => _,
            in("rax") 0u64,
            options(nostack, preserves_flags),
        )
    }
}

/// Copies the `pages` pages (4 KiB each, at least one) at `src` to `dst`,
/// in one `rep movsq`.
///
/// # Safety
/// `src` is readable, `dst` writable and owned by the caller, any
/// alignment, and the ranges do not overlap.
#[inline]
pub unsafe fn copy_pages(dst: *mut u8, src: *const u8, pages: usize) {
    // SAFETY: guaranteed by the caller; as for `zero_page`.
    unsafe {
        asm!(
            "rep movsq",
            inout("rcx") pages * PAGE_QWORDS => _,
            inout("rdi") dst => _,
            inout("rsi") src => _,
            options(nostack, preserves_flags),
        )
    }
}

/// Whether the CPU has SSE4.2's `crc32`: 0 not known yet, 1 yes, 2 no.
/// Kept because `CPUID` exits to the hypervisor.
#[cfg(feature = "cells")]
static CRC: AtomicU8 = AtomicU8::new(0);

/// Whether `CPUID` lists SSE4.2 (leaf 1, ECX bit 20).
#[cfg(feature = "cells")]
pub fn has_crc32() -> bool {
    match CRC.load(Ordering::Relaxed) {
        0 => {
            let present = core::arch::x86_64::__cpuid(1).ecx & (1 << 20) != 0;
            CRC.store(if present { 1 } else { 2 }, Ordering::Relaxed);
            present
        }
        known => known == 1,
    }
}

/// CRC-32C of `bytes` (as `rhizo_snapshot::crc32c`): SSE4.2's `crc32`
/// where the CPU has it, else the portable table.
#[cfg(feature = "cells")]
pub fn crc32c(bytes: &[u8]) -> u32 {
    if !has_crc32() {
        return rhizo_snapshot::crc32c(bytes);
    }
    let mut c = u64::from(!0u32);
    let (blocks, words) = (bytes.len() / 32, bytes.len() % 32 / 8);
    let mut p = bytes.as_ptr();
    if blocks > 0 {
        // SAFETY: only reached when the CPU has SSE4.2; it reads the
        // `32 * blocks` bytes at `p`, inside `bytes`. Four words per loop:
        // fewer instructions around each `crc32`.
        unsafe {
            asm!(
                "2:",
                "crc32 {c}, qword ptr [{p}]",
                "crc32 {c}, qword ptr [{p} + 8]",
                "crc32 {c}, qword ptr [{p} + 16]",
                "crc32 {c}, qword ptr [{p} + 24]",
                "add {p}, 32",
                "cmp {p}, {end}",
                "jne 2b",
                end = in(reg) p.add(32 * blocks),
                p = inout(reg) p,
                c = inout(reg) c,
                options(readonly, nostack),
            )
        };
    }
    for _ in 0..words {
        // SAFETY: as above, for the next 8 bytes of `bytes`.
        unsafe {
            asm!(
                "crc32 {c}, qword ptr [{p}]",
                "add {p}, 8",
                p = inout(reg) p,
                c = inout(reg) c,
                options(readonly, nostack),
            )
        };
    }
    let mut c = c as u32;
    for &b in &bytes[bytes.len() / 8 * 8..] {
        // SAFETY: as above; registers only.
        unsafe {
            asm!(
                "crc32 {c:e}, {v}",
                c = inout(reg) c,
                v = in(reg_byte) b,
                options(pure, nomem, nostack),
            )
        };
    }
    !c
}
