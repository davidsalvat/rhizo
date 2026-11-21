//! Hardware entropy: `RNDR` (FEAT_RNG), when the CPU has it.

use core::arch::asm;
use rhizo_hal::Entropy;

/// The CPU's random-number register.
#[derive(Debug)]
pub struct HardwareEntropy;

/// Tries per 64-bit word: `RNDR` fails only when it cannot produce a value
/// in reasonable time (Arm ARM), so a few tries suffice.
const TRIES: usize = 10;

fn present() -> bool {
    let isar0: u64;
    // SAFETY: ID_AA64ISAR0_EL1 is readable at EL1 and has no side effects.
    unsafe {
        asm!("mrs {}, id_aa64isar0_el1", out(reg) isar0, options(nomem, nostack, preserves_flags))
    };
    (isar0 >> 60) & 0xf != 0
}

fn rndr() -> Option<u64> {
    let (value, ok): (u64, u64);
    // SAFETY: only called when ID_AA64ISAR0_EL1 lists RNDR (encoded as
    // `s3_3_c2_c4_0`); it writes a register and NZCV (Z set on failure).
    unsafe {
        asm!("mrs {}, s3_3_c2_c4_0", "cset {}, ne", out(reg) value, out(reg) ok, options(nomem, nostack))
    };
    (ok != 0).then_some(value)
}

impl Entropy for HardwareEntropy {
    fn fill(&mut self, out: &mut [u8]) -> bool {
        if !present() {
            return false;
        }
        for chunk in out.chunks_mut(8) {
            let Some(word) = (0..TRIES).find_map(|_| rndr()) else {
                return false;
            };
            chunk.copy_from_slice(&word.to_le_bytes()[..chunk.len()]);
        }
        true
    }
}
