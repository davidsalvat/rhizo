//! Hardware entropy: `RDSEED` (the entropy source itself), else `RDRAND`
//! (a generator the source reseeds), as Intel's guide recommends.

use core::arch::asm;
use core::arch::x86_64::{__cpuid, __cpuid_count};
use rhizo_hal::Entropy;

/// The CPU's random-number instructions.
#[derive(Debug)]
pub struct HardwareEntropy;

/// Tries per 64-bit word: `RDRAND` fails only when its generator is
/// drained, for a moment (Intel's guide: ten tries); `RDSEED` fails more
/// often under load.
const RDRAND_TRIES: usize = 10;
const RDSEED_TRIES: usize = 100;

fn rdseed_present() -> bool {
    // CPUID leaf 0 always exists; leaf 7 only when leaf 0 says so.
    __cpuid(0).eax >= 7 && __cpuid_count(7, 0).ebx & (1 << 18) != 0
}

fn rdrand_present() -> bool {
    __cpuid(1).ecx & (1 << 30) != 0
}

fn rdseed() -> Option<u64> {
    let (value, ok): (u64, u8);
    // SAFETY: only called when CPUID lists RDSEED; it writes a register and
    // the carry flag.
    unsafe {
        asm!("rdseed {}", "setc {}", out(reg) value, out(reg_byte) ok, options(nomem, nostack))
    };
    (ok != 0).then_some(value)
}

fn rdrand() -> Option<u64> {
    let (value, ok): (u64, u8);
    // SAFETY: only called when CPUID lists RDRAND; as for `rdseed`.
    unsafe {
        asm!("rdrand {}", "setc {}", out(reg) value, out(reg_byte) ok, options(nomem, nostack))
    };
    (ok != 0).then_some(value)
}

/// One word: from `RDSEED` if it has one, else from `RDRAND`.
fn word(seed: bool, rand: bool) -> Option<u64> {
    let from_seed = || {
        (0..RDSEED_TRIES).find_map(|_| {
            core::hint::spin_loop();
            rdseed()
        })
    };
    let from_rand = || (0..RDRAND_TRIES).find_map(|_| rdrand());
    seed.then(from_seed)
        .flatten()
        .or_else(|| rand.then(from_rand).flatten())
}

impl Entropy for HardwareEntropy {
    fn fill(&mut self, out: &mut [u8]) -> bool {
        let (seed, rand) = (rdseed_present(), rdrand_present());
        for chunk in out.chunks_mut(8) {
            let Some(word) = word(seed, rand) else {
                return false;
            };
            chunk.copy_from_slice(&word.to_le_bytes()[..chunk.len()]);
        }
        true
    }
}
