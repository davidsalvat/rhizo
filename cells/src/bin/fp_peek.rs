//! A hostile reader of the FP/SIMD registers (change `lazy-fp-switch`): it
//! never writes them, only reads them, first as its very first FP/SIMD
//! instruction and then after each of `n` yields (argument 1, default
//! 100) while other cells hold their own values in theirs. It must only
//! ever find the state a program starts with (all zero; on x86_64 the x87
//! control word `0x37f` and `MXCSR` `0x1f80`), as on Linux: anything else
//! came from another thread or cell.
#![no_std]
#![no_main]

use rhizo_cells::{arg, number, print, yield_now};

/// The registers as `FXSAVE` writes them on x86_64; `V0`-`V31`, `FPCR`,
/// `FPSR` on AArch64.
#[repr(C, align(16))]
struct Image([u64; 66]);

/// Reads the FP/SIMD registers without changing them.
fn read() -> Image {
    let mut image = Image([0; 66]);
    let at = image.0.as_mut_ptr();
    // SAFETY: `FXSAVE` writes 512 bytes at a 16-byte aligned address, which
    // `Image` is; it changes no register.
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("fxsave64 [{}]", in(reg) at, options(nostack));
    }
    // SAFETY: the stores write 528 bytes into `image`; they change no
    // register but the two scratch ones named.
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!(
            ".arch_extension fp",
            ".arch_extension simd",
            "stp q0, q1, [{a}, #0]",
            "stp q2, q3, [{a}, #32]",
            "stp q4, q5, [{a}, #64]",
            "stp q6, q7, [{a}, #96]",
            "stp q8, q9, [{a}, #128]",
            "stp q10, q11, [{a}, #160]",
            "stp q12, q13, [{a}, #192]",
            "stp q14, q15, [{a}, #224]",
            "stp q16, q17, [{a}, #256]",
            "stp q18, q19, [{a}, #288]",
            "stp q20, q21, [{a}, #320]",
            "stp q22, q23, [{a}, #352]",
            "stp q24, q25, [{a}, #384]",
            "stp q26, q27, [{a}, #416]",
            "stp q28, q29, [{a}, #448]",
            "stp q30, q31, [{a}, #480]",
            "mrs {c}, fpcr",
            "mrs {s}, fpsr",
            "str {c}, [{a}, #512]",
            "str {s}, [{a}, #520]",
            a = in(reg) at,
            c = out(reg) _,
            s = out(reg) _,
            options(nostack)
        );
    }
    image
}

/// Whether `image` is the state a program starts with.
fn initial(image: &Image) -> bool {
    let w = &image.0;
    if cfg!(target_arch = "x86_64") {
        // x87 control word and MXCSR; the x87 registers (bytes 32-159) and
        // XMM0-XMM15 (bytes 160-415) zero.
        w[0] & 0xffff == 0x37f && w[3] & 0xffff_ffff == 0x1f80 && w[4..52].iter().all(|&x| x == 0)
    } else {
        w.iter().all(|&x| x == 0)
    }
}

fn main() -> i32 {
    let rounds = arg(1).map_or(100, number);
    if !initial(&read()) {
        print(b"fp LEAK at start\n");
        return 1;
    }
    for _ in 0..rounds {
        yield_now();
        if !initial(&read()) {
            print(b"fp LEAK after a yield\n");
            return 1;
        }
    }
    print(b"fp clean\n");
    0
}

rhizo_cells::entry!(main);
