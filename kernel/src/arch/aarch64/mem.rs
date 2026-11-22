//! Page-sized zero and copy, and CRC-32C, with the instructions made for
//! them (change `fast-mem-primitives`): `DC ZVA` zeroes a cache-line block
//! at a time, the copy moves 128 bytes per loop in pairs of x registers
//! (the kernel is built soft-float: no SIMD register is touched, so a
//! cell's are never clobbered), and the `CRC32C*` instructions checksum
//! eight bytes each. The kernel's `memcpy`, `memmove`, `memset` and
//! `memcmp` are here too (change `fast-thread-switch-aarch64`), in place
//! of `compiler_builtins`' word loops.

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

// The kernel's `memcpy`, `memmove`, `memset` and `memcmp` (change
// `fast-thread-switch-aarch64`, design D1). `compiler_builtins` defines
// them as weak symbols with 8-byte loops (byte loops when the source and
// destination are aligned differently, and for `memcmp`); these strong
// definitions replace them for every copy the compiler emits, thread
// records and socket buffers included. General registers only (x0–x17,
// caller-saved; no SIMD: the kernel is built soft-float and a cell's
// vector registers stay live in the CPU), and unaligned accesses, which
// normal memory allows (`SCTLR_EL1.A` is clear, and the kernel runs Rust
// code only with the MMU on). Every size class loads all of its bytes
// before it stores any, so `memmove` reuses them in either direction.
//
// Copies: 64 bytes per loop in four `LDP`/`STP` pairs, then the rest
// (0–63 bytes) by size class with overlapping accesses at both ends:
// 1–3 (bytes at 0, n/2, n-1), 4–7, 8–16, 17–32, 33–63.
core::arch::global_asm!(
    r#"
    .section .text.memcpy, "ax", %progbits
    .p2align 4
    .global memcpy
    .type memcpy, %function
memcpy:
    mov     x3, x0
.Lcopy_forward:
    // x3: destination, x1: source, x2: length.
    subs    x12, x2, #64
    b.lo    .Lcopy_small
    add     x12, x1, x12            // the last source address a loop may start at
.Lcopy_forward_loop:
    ldp     x4, x5, [x1], #64
    ldp     x6, x7, [x1, #-48]
    ldp     x8, x9, [x1, #-32]
    ldp     x10, x11, [x1, #-16]
    stp     x4, x5, [x3], #64
    stp     x6, x7, [x3, #-48]
    stp     x8, x9, [x3, #-32]
    stp     x10, x11, [x3, #-16]
    cmp     x1, x12
    b.ls    .Lcopy_forward_loop
    sub     x2, x12, x1
    add     x2, x2, #64
.Lcopy_small:
    // x3: destination, x1: source, x2: length below 64.
    add     x12, x1, x2
    add     x13, x3, x2
    cmp     x2, #16
    b.hi    .Lcopy_17_63
    cmp     x2, #8
    b.hs    .Lcopy_8_16
    cmp     x2, #4
    b.hs    .Lcopy_4_7
    cbz     x2, .Lcopy_done
    lsr     x4, x2, #1
    ldrb    w5, [x1]
    ldrb    w6, [x1, x4]
    ldurb   w7, [x12, #-1]
    strb    w5, [x3]
    strb    w6, [x3, x4]
    sturb   w7, [x13, #-1]
.Lcopy_done:
    ret
.Lcopy_4_7:
    ldr     w4, [x1]
    ldur    w5, [x12, #-4]
    str     w4, [x3]
    stur    w5, [x13, #-4]
    ret
.Lcopy_8_16:
    ldr     x4, [x1]
    ldur    x5, [x12, #-8]
    str     x4, [x3]
    stur    x5, [x13, #-8]
    ret
.Lcopy_17_63:
    cmp     x2, #32
    b.hi    .Lcopy_33_63
    ldp     x4, x5, [x1]
    ldp     x6, x7, [x12, #-16]
    stp     x4, x5, [x3]
    stp     x6, x7, [x13, #-16]
    ret
.Lcopy_33_63:
    ldp     x4, x5, [x1]
    ldp     x6, x7, [x1, #16]
    ldp     x8, x9, [x12, #-32]
    ldp     x10, x11, [x12, #-16]
    stp     x4, x5, [x3]
    stp     x6, x7, [x3, #16]
    stp     x8, x9, [x13, #-32]
    stp     x10, x11, [x13, #-16]
    ret
    .size memcpy, . - memcpy

    // memmove: forward unless the destination starts inside the source
    // (dst - src < n, unsigned), then backward from the ends, 64 bytes per
    // loop, and the head as a small copy. Each loop step loads its 64
    // bytes before storing them, and moves away from what it stored, so
    // overlapping ranges are copied as if through a buffer.
    .global memmove
    .type memmove, %function
memmove:
    mov     x3, x0
    sub     x4, x0, x1
    cmp     x4, x2
    b.hs    .Lcopy_forward
    cmp     x2, #64
    b.lo    .Lcopy_small
    add     x12, x1, x2
    add     x13, x0, x2
    add     x14, x1, #64            // the loop runs while 64 bytes are left
.Lmove_backward_loop:
    ldp     x4, x5, [x12, #-16]
    ldp     x6, x7, [x12, #-32]
    ldp     x8, x9, [x12, #-48]
    ldp     x10, x11, [x12, #-64]!
    stp     x4, x5, [x13, #-16]
    stp     x6, x7, [x13, #-32]
    stp     x8, x9, [x13, #-48]
    stp     x10, x11, [x13, #-64]!
    cmp     x12, x14
    b.hs    .Lmove_backward_loop
    sub     x2, x12, x1
    b       .Lcopy_small
    .size memmove, . - memmove

    // memset: the byte in all eight lanes of x1, then 64 bytes per loop
    // and the last 64 at the end (overlapping), or one size class.
    .section .text.memset, "ax", %progbits
    .p2align 4
    .global memset
    .type memset, %function
memset:
    and     x1, x1, #0xff
    mov     x4, #0x0101010101010101
    mul     x1, x1, x4
    add     x13, x0, x2
    cmp     x2, #16
    b.hi    .Lset_17
    cmp     x2, #8
    b.hs    .Lset_8_16
    cmp     x2, #4
    b.hs    .Lset_4_7
    cbz     x2, .Lset_done
    lsr     x4, x2, #1
    strb    w1, [x0]
    strb    w1, [x0, x4]
    sturb   w1, [x13, #-1]
.Lset_done:
    ret
.Lset_4_7:
    str     w1, [x0]
    stur    w1, [x13, #-4]
    ret
.Lset_8_16:
    str     x1, [x0]
    stur    x1, [x13, #-8]
    ret
.Lset_17:
    cmp     x2, #32
    b.hi    .Lset_33
    stp     x1, x1, [x0]
    stp     x1, x1, [x13, #-16]
    ret
.Lset_33:
    cmp     x2, #64
    b.hi    .Lset_large
    stp     x1, x1, [x0]
    stp     x1, x1, [x0, #16]
    stp     x1, x1, [x13, #-32]
    stp     x1, x1, [x13, #-16]
    ret
.Lset_large:
    mov     x3, x0
    sub     x12, x13, #64           // the last 64 bytes, stored after the loop
.Lset_loop:
    stp     x1, x1, [x3], #64
    stp     x1, x1, [x3, #-48]
    stp     x1, x1, [x3, #-32]
    stp     x1, x1, [x3, #-16]
    cmp     x3, x12
    b.lo    .Lset_loop
    stp     x1, x1, [x12]
    stp     x1, x1, [x12, #16]
    stp     x1, x1, [x12, #32]
    stp     x1, x1, [x12, #48]
    ret
    .size memset, . - memset

    // memcmp: 16 bytes per loop, then 8, then single bytes; the first
    // differing word, byte-reversed, compares as the bytes do (the
    // lowest address is the most significant).
    .section .text.memcmp, "ax", %progbits
    .p2align 4
    .global memcmp
    .type memcmp, %function
memcmp:
    subs    x2, x2, #16
    b.lo    .Lcmp_8
.Lcmp_loop:
    ldp     x4, x5, [x0], #16
    ldp     x6, x7, [x1], #16
    cmp     x4, x6
    ccmp    x5, x7, #0, eq
    b.ne    .Lcmp_pair
    subs    x2, x2, #16
    b.hs    .Lcmp_loop
.Lcmp_8:
    adds    x2, x2, #8
    b.lo    .Lcmp_bytes
    ldr     x4, [x0], #8
    ldr     x6, [x1], #8
    cmp     x4, x6
    b.ne    .Lcmp_word
    sub     x2, x2, #8
.Lcmp_bytes:
    adds    x2, x2, #8              // 0-7 bytes left
    b.eq    .Lcmp_equal
.Lcmp_byte_loop:
    ldrb    w4, [x0], #1
    ldrb    w6, [x1], #1
    subs    w4, w4, w6
    b.ne    .Lcmp_byte_differs
    subs    x2, x2, #1
    b.ne    .Lcmp_byte_loop
.Lcmp_equal:
    mov     w0, #0
    ret
.Lcmp_byte_differs:
    mov     w0, w4
    ret
.Lcmp_pair:
    cmp     x4, x6
    csel    x4, x4, x5, ne
    csel    x6, x6, x7, ne
.Lcmp_word:
    rev     x4, x4
    rev     x6, x6
    cmp     x4, x6
    mov     w0, #1
    cneg    w0, w0, lo
    ret
    .size memcmp, . - memcmp
"#
);
