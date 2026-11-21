//! Cryptographic random numbers (spec `isolation`, design D1 of
//! `add-crypto-random`): the ChaCha20 block function (RFC 8439) run as a
//! fast-key-erasure generator. Every request first replaces the key with
//! key stream, so what was handed out before cannot be computed again from
//! the state, and a generator split off another shares nothing with it.
//!
//! Seeds come in through [`rhizo_hal::Entropy`]: the logic here is
//! deterministic and tested on the host.

use core::fmt;
use rhizo_hal::Entropy;

/// "expand 32-byte k" (RFC 8439, section 2.3).
const CONSTANTS: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];
/// A generator's key never encrypts anything: one nonce is enough.
const NONCE: [u8; 12] = [0; 12];

fn quarter_round(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(7);
}

fn word(bytes: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([
        bytes[4 * i],
        bytes[4 * i + 1],
        bytes[4 * i + 2],
        bytes[4 * i + 3],
    ])
}

/// The ChaCha20 block function (RFC 8439, section 2.3): 64 bytes of key
/// stream for `key`, block `counter` and `nonce`.
pub fn block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut init = [0u32; 16];
    init[..4].copy_from_slice(&CONSTANTS);
    for i in 0..8 {
        init[4 + i] = word(key, i);
    }
    init[12] = counter;
    for i in 0..3 {
        init[13 + i] = word(nonce, i);
    }
    let mut s = init;
    for _ in 0..10 {
        quarter_round(&mut s, 0, 4, 8, 12);
        quarter_round(&mut s, 1, 5, 9, 13);
        quarter_round(&mut s, 2, 6, 10, 14);
        quarter_round(&mut s, 3, 7, 11, 15);
        quarter_round(&mut s, 0, 5, 10, 15);
        quarter_round(&mut s, 1, 6, 11, 12);
        quarter_round(&mut s, 2, 7, 8, 13);
        quarter_round(&mut s, 3, 4, 9, 14);
    }
    let mut out = [0u8; 64];
    for (i, chunk) in out.chunks_exact_mut(4).enumerate() {
        chunk.copy_from_slice(&s[i].wrapping_add(init[i]).to_le_bytes());
    }
    out
}

/// A ChaCha20 generator with fast key erasure. Deliberately not `Clone`:
/// two copies would hand out the same bytes (use [`split`](Self::split)).
pub struct Generator {
    key: [u8; 32],
}

impl fmt::Debug for Generator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Generator { .. }") // never print the key
    }
}

impl Generator {
    /// A generator keyed with `key`.
    pub const fn from_key(key: [u8; 32]) -> Self {
        Self { key }
    }

    /// Fills `out` with key stream and replaces the key: block 0 gives the
    /// next key (its first 32 bytes) and the first 32 output bytes, blocks
    /// 1, 2, ... the rest. A request is at most 256 GiB (the block counter
    /// is 32 bits); the kernel's are at most a page.
    pub fn fill(&mut self, out: &mut [u8]) {
        let first = block(&self.key, 0, &NONCE);
        let n = out.len().min(32);
        out[..n].copy_from_slice(&first[32..32 + n]);
        for (i, chunk) in out[n..].chunks_mut(64).enumerate() {
            let next = block(&self.key, i as u32 + 1, &NONCE);
            chunk.copy_from_slice(&next[..chunk.len()]);
        }
        self.key.copy_from_slice(&first[..32]);
    }

    /// A new generator keyed with 32 bytes of this one's output: neither can
    /// compute the other's output (design D2).
    pub fn split(&mut self) -> Self {
        let mut key = [0; 32];
        self.fill(&mut key);
        Self { key }
    }
}

/// Bytes a [`Batched`] keeps ready: drawn at once, four ChaCha20 blocks (one
/// gives the next key).
const BATCH: usize = 192;

/// A [`Generator`] that draws [`BATCH`] bytes at a time and hands out small
/// requests from them (design D1): the kernel's own generator, whose
/// requests are 16 or 32 bytes (`AT_RANDOM`, a cell's generator), so a
/// spawn does not pay a whole ChaCha20 block. A byte is erased as it is
/// handed out, so what the state holds was never given to anyone.
pub struct Batched {
    generator: Generator,
    ready: [u8; BATCH],
    /// Bytes of `ready` already handed out (and zeroed), from the start.
    used: usize,
}

impl fmt::Debug for Batched {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Batched { .. }") // never print the key or the bytes
    }
}

impl Batched {
    /// A pool drawing from `generator`.
    pub const fn new(generator: Generator) -> Self {
        Self {
            generator,
            ready: [0; BATCH],
            used: BATCH,
        }
    }

    /// Fills `out`: from the bytes kept ready when it fits in a batch,
    /// straight from the generator otherwise.
    pub fn fill(&mut self, out: &mut [u8]) {
        if out.len() > BATCH {
            return self.generator.fill(out);
        }
        if BATCH - self.used < out.len() {
            self.generator.fill(&mut self.ready);
            self.used = 0;
        }
        let taken = &mut self.ready[self.used..self.used + out.len()];
        out.copy_from_slice(taken);
        taken.fill(0);
        self.used += out.len();
    }

    /// A new generator keyed with 32 bytes of this pool's output.
    pub fn split(&mut self) -> Generator {
        let mut key = [0; 32];
        self.fill(&mut key);
        Generator::from_key(key)
    }
}

/// A generator seeded from `entropy`, and whether the source gave
/// anything. Without it, the seed is `fallback` (a cycle count, say):
/// predictable, so the caller must say so.
pub fn seeded(entropy: &mut impl Entropy, fallback: u64) -> (Generator, bool) {
    let mut key = [0; 32];
    let strong = entropy.fill(&mut key);
    if !strong {
        key = [0; 32];
        key[..8].copy_from_slice(&fallback.to_le_bytes());
    }
    let mut generator = Generator::from_key(key);
    generator.fill(&mut []); // the seed itself is never a key in use
    (generator, strong)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhizo_hal::testing::FakeEntropy;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn block_function_matches_rfc_8439() {
        // RFC 8439, section 2.3.2.
        let key: [u8; 32] = core::array::from_fn(|i| i as u8);
        let nonce = [0, 0, 0, 9, 0, 0, 0, 0x4a, 0, 0, 0, 0];
        assert_eq!(
            hex(&block(&key, 1, &nonce)),
            "10f1e7e4d13b5915500fdd1fa32071c4c7d1f4c733c068030422aa9ac3d46c4e\
             d2826446079faa0914c2d705d98b02a2b5129cd1de164eb9cbd083e8a2503c4e"
        );
        // RFC 8439, appendix A.1, test vector 1.
        assert_eq!(
            hex(&block(&[0; 32], 0, &[0; 12])),
            "76b8e0ada0f13d90405d6ae55386bd28bdd219b8a08ded1aa836efcc8b770dc7\
             da41597c5157488d7724e03fb8d84a376a43b8f41518a11cc387b669b2ee6586"
        );
    }

    #[test]
    fn output_is_the_key_stream_after_the_next_key() {
        let key = [7; 32];
        let mut g = Generator::from_key(key);
        let mut out = [0; 100];
        g.fill(&mut out);
        let b0 = block(&key, 0, &NONCE);
        let b1 = block(&key, 1, &NONCE);
        assert_eq!(out[..32], b0[32..]);
        assert_eq!(out[32..96], b1[..]);
        assert_eq!(out[96..], block(&key, 2, &NONCE)[..4]);
        assert_eq!(g.key[..], b0[..32], "the next key is block 0's first half");
    }

    #[test]
    fn the_key_is_erased_after_every_request() {
        let mut g = Generator::from_key([1; 32]);
        let mut first = [0; 32];
        g.fill(&mut first);
        // Whoever reads the state now cannot get `first` back from it.
        let mut again = Generator::from_key(g.key);
        let mut out = [0; 32];
        again.fill(&mut out);
        assert_ne!(out, first);
        let mut second = [0; 32];
        g.fill(&mut second);
        assert_ne!(first, second);
        // An empty request still moves the key on.
        let before = g.key;
        g.fill(&mut []);
        assert_ne!(g.key, before);
    }

    #[test]
    fn split_generators_share_no_output() {
        let mut pool = Generator::from_key([3; 32]);
        let mut a = pool.split();
        let mut b = pool.split();
        let (mut x, mut y, mut z) = ([0; 64], [0; 64], [0; 64]);
        a.fill(&mut x);
        b.fill(&mut y);
        pool.fill(&mut z);
        assert_ne!(x, y);
        assert_ne!(x, z);
        assert_ne!(y, z);
    }

    #[test]
    fn same_seed_same_output_so_it_is_testable() {
        let (mut a, strong) = seeded(&mut FakeEntropy(Some(5)), 0);
        assert!(strong);
        let (mut b, _) = seeded(&mut FakeEntropy(Some(5)), 0);
        let (mut c, _) = seeded(&mut FakeEntropy(Some(6)), 0);
        let (mut x, mut y, mut z) = ([0; 16], [0; 16], [0; 16]);
        a.fill(&mut x);
        b.fill(&mut y);
        c.fill(&mut z);
        assert_eq!(x, y);
        assert_ne!(x, z);
    }

    #[test]
    fn a_missing_source_is_reported_and_the_fallback_used() {
        let (mut a, strong) = seeded(&mut FakeEntropy(None), 42);
        assert!(!strong);
        let (mut b, _) = seeded(&mut FakeEntropy(None), 43);
        let (mut x, mut y) = ([0; 16], [0; 16]);
        a.fill(&mut x);
        b.fill(&mut y);
        assert_ne!(x, y);
    }

    #[test]
    fn a_batch_hands_out_the_generator_output_once() {
        let mut pool = Batched::new(Generator::from_key([9; 32]));
        let mut stream = Generator::from_key([9; 32]);
        let mut expected = [0; BATCH];
        stream.fill(&mut expected);
        let mut got = [0; BATCH];
        for chunk in got.chunks_mut(16) {
            pool.fill(chunk);
        }
        assert_eq!(got, expected, "the batch, in order, without repeats");
        assert!(
            pool.ready.iter().all(|&b| b == 0),
            "handed-out bytes are erased"
        );
        let mut next = [0; 16];
        pool.fill(&mut next);
        let mut again = [0; BATCH];
        stream.fill(&mut again);
        assert_eq!(next[..], again[..16], "a new batch under the next key");
    }

    #[test]
    fn a_request_that_does_not_fit_is_not_split_across_batches() {
        let mut pool = Batched::new(Generator::from_key([4; 32]));
        let mut a = [0; 180];
        pool.fill(&mut a);
        let mut b = [0; 32]; // 12 bytes left: a new batch
        pool.fill(&mut b);
        assert!(!a.windows(32).any(|w| w == b));
        let mut big = [0; BATCH + 1];
        pool.fill(&mut big); // straight from the generator
        assert_eq!(pool.used, 32, "the batch kept ready is left alone");
        assert!(!big.windows(32).any(|w| w == b));
    }

    #[test]
    fn pools_split_generators_that_share_no_output() {
        let mut pool = Batched::new(Generator::from_key([5; 32]));
        let (mut a, mut b) = (pool.split(), pool.split());
        let (mut x, mut y, mut z) = ([0; 32], [0; 32], [0; 32]);
        a.fill(&mut x);
        b.fill(&mut y);
        pool.fill(&mut z);
        assert_ne!(x, y);
        assert_ne!(x, z);
        assert_ne!(y, z);
    }

    #[test]
    fn debug_never_shows_the_key() {
        assert_eq!(
            format!("{:?}", Batched::new(Generator::from_key([0xab; 32]))),
            "Batched { .. }"
        );
        assert_eq!(
            format!("{:?}", Generator::from_key([0xab; 32])),
            "Generator { .. }"
        );
    }
}
