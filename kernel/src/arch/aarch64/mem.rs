//! Page-sized zero and copy, and CRC-32C, with the instructions made for
//! them (change `fast-mem-primitives`): `DC ZVA` zeroes a cache-line block
//! at a time, the copy moves 128 bytes per loop in pairs of x registers
//! (the kernel is built soft-float: no SIMD register is touched, so a
//! cell's are never clobbered), and the `CRC32C*` instructions checksum
//! eight bytes each.

use core::arch::asm;
#[cfg(feature = "cells")]
use core::sync::atomic::{AtomicU8, Ordering};

/// Bytes of a page, the unit of both primitives.
const PAGE: usize = 4096;

/// The `DC ZVA` block in bytes, or `None` where it is prohibited
/// (`DCZID_EL0.DZP`).
fn zva_block() -> Option<usize> {
    let dczid: u64;
    // SAFETY: DCZID_EL0 is readable at every exception level and has no
    // side effects.
    unsafe { asm!("mrs {}, dczid_el0", out(reg) dczid, options(nomem, nostack, preserves_flags)) };
    // BS is log2 of the block in 4-byte words; the Arm ARM caps it at
    // 2 KiB, so a page is a whole number of blocks.
    (dczid & 0x10 == 0).then(|| 4 << (dczid & 0xf))
}

/// Fills the page at `dst` with zeros.
///
/// # Safety
/// `dst` is 4 KiB-aligned, writable normal memory (`DC ZVA` faults on
/// device memory), and owned by the caller for the call.
#[inline]
pub unsafe fn zero_page(dst: *mut u8) {
    match zva_block() {
        // SAFETY: the page is the caller's normal memory, a whole number
        // of blocks; `DC ZVA` zeroes the aligned block holding the address.
        Some(block) => unsafe {
            asm!(
                "2:",
                "dc zva, {p}",
                "add {p}, {p}, {b}",
                "cmp {p}, {end}",
                "b.ne 2b",
                p = inout(reg) dst => _,
                b = in(reg) block,
                end = in(reg) dst.add(PAGE),
                options(nostack),
            )
        },
        // SAFETY: as above: 32 stores of 16 bytes per loop, 8 loops.
        None => unsafe {
            asm!(
                "2:",
                "stp xzr, xzr, [{p}]",
                "stp xzr, xzr, [{p}, #16]",
                "stp xzr, xzr, [{p}, #32]",
                "stp xzr, xzr, [{p}, #48]",
                "stp xzr, xzr, [{p}, #64]",
                "stp xzr, xzr, [{p}, #80]",
                "stp xzr, xzr, [{p}, #96]",
                "stp xzr, xzr, [{p}, #112]",
                "add {p}, {p}, #128",
                "cmp {p}, {end}",
                "b.ne 2b",
                p = inout(reg) dst => _,
                end = in(reg) dst.add(PAGE),
                options(nostack),
            )
        },
    }
}

/// Copies the `pages` pages (4 KiB each, at least one) at `src` to `dst`.
///
/// # Safety
/// Both ranges are normal memory (any alignment: `SCTLR_EL1.A` is clear),
/// `src` readable, `dst` writable and owned by the caller, and they do not
/// overlap.
#[inline]
pub unsafe fn copy_pages(dst: *mut u8, src: *const u8, pages: usize) {
    // SAFETY: guaranteed by the caller; 128 bytes per loop, 32 loops a
    // page, in eight x registers (no SIMD).
    unsafe {
        asm!(
            "2:",
            "ldp {a}, {b}, [{s}]",
            "ldp {c}, {d}, [{s}, #16]",
            "ldp {e}, {f}, [{s}, #32]",
            "ldp {g}, {h}, [{s}, #48]",
            "stp {a}, {b}, [{d_}]",
            "stp {c}, {d}, [{d_}, #16]",
            "stp {e}, {f}, [{d_}, #32]",
            "stp {g}, {h}, [{d_}, #48]",
            "ldp {a}, {b}, [{s}, #64]",
            "ldp {c}, {d}, [{s}, #80]",
            "ldp {e}, {f}, [{s}, #96]",
            "ldp {g}, {h}, [{s}, #112]",
            "add {s}, {s}, #128",
            "stp {a}, {b}, [{d_}, #64]",
            "stp {c}, {d}, [{d_}, #80]",
            "stp {e}, {f}, [{d_}, #96]",
            "stp {g}, {h}, [{d_}, #112]",
            "add {d_}, {d_}, #128",
            "cmp {s}, {end}",
            "b.ne 2b",
            s = inout(reg) src => _,
            d_ = inout(reg) dst => _,
            end = in(reg) src.add(pages * PAGE),
            a = out(reg) _,
            b = out(reg) _,
            c = out(reg) _,
            d = out(reg) _,
            e = out(reg) _,
            f = out(reg) _,
            g = out(reg) _,
            h = out(reg) _,
            options(nostack),
        )
    }
}

/// Whether the CPU has the CRC32 instructions: 0 not known yet, 1 yes,
/// 2 no. Kept because hypervisors may trap reads of ID registers.
#[cfg(feature = "cells")]
static CRC: AtomicU8 = AtomicU8::new(0);

/// Whether `ID_AA64ISAR0_EL1` lists the CRC32 instructions.
#[cfg(feature = "cells")]
pub fn has_crc32() -> bool {
    match CRC.load(Ordering::Relaxed) {
        0 => {
            let isar0: u64;
            // SAFETY: ID_AA64ISAR0_EL1 is readable at EL1 and has no side
            // effects.
            unsafe {
                asm!("mrs {}, id_aa64isar0_el1", out(reg) isar0, options(nomem, nostack, preserves_flags))
            };
            let present = (isar0 >> 16) & 0xf != 0;
            CRC.store(if present { 1 } else { 2 }, Ordering::Relaxed);
            present
        }
        known => known == 1,
    }
}

/// CRC-32C of `bytes` (as `rhizo_snapshot::crc32c`): the `CRC32C*`
/// instructions where the CPU has them, else the portable table.
#[cfg(feature = "cells")]
pub fn crc32c(bytes: &[u8]) -> u32 {
    if !has_crc32() {
        return rhizo_snapshot::crc32c(bytes);
    }
    // The loads are in the assembly: the target is built `strict-align`,
    // so the compiler would assemble an unaligned word from single bytes,
    // while the CPU loads it at once from normal memory (`SCTLR_EL1.A` is
    // clear).
    let mut c = !0u32;
    let (blocks, words) = (bytes.len() / 32, bytes.len() % 32 / 8);
    let mut p = bytes.as_ptr();
    if blocks > 0 {
        // SAFETY: only reached when the CPU has the CRC32 instructions;
        // it reads the `32 * blocks` bytes at `p`, inside `bytes`.
        unsafe {
            asm!(
                ".arch_extension crc",
                "2:",
                "ldp {a}, {b}, [{p}], #16",
                "ldp {x}, {y}, [{p}], #16",
                "crc32cx {c:w}, {c:w}, {a}",
                "crc32cx {c:w}, {c:w}, {b}",
                "crc32cx {c:w}, {c:w}, {x}",
                "crc32cx {c:w}, {c:w}, {y}",
                "cmp {p}, {end}",
                "b.ne 2b",
                end = in(reg) p.add(32 * blocks),
                p = inout(reg) p,
                c = inout(reg) c,
                a = out(reg) _,
                b = out(reg) _,
                x = out(reg) _,
                y = out(reg) _,
                options(readonly, nostack),
            )
        };
    }
    for _ in 0..words {
        // SAFETY: as above, for the next 8 bytes of `bytes`.
        unsafe {
            asm!(
                ".arch_extension crc",
                "ldr {v}, [{p}], #8",
                "crc32cx {c:w}, {c:w}, {v}",
                p = inout(reg) p,
                c = inout(reg) c,
                v = out(reg) _,
                options(readonly, nostack, preserves_flags),
            )
        };
    }
    for &b in &bytes[bytes.len() / 8 * 8..] {
        // SAFETY: as for `word`.
        unsafe {
            asm!(
                ".arch_extension crc",
                "crc32cb {c:w}, {c:w}, {v:w}",
                c = inout(reg) c,
                v = in(reg) b as u32,
                options(pure, nomem, nostack, preserves_flags),
            )
        };
    }
    !c
}
