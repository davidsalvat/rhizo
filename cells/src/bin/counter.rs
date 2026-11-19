//! A deterministic workload whose state lives everywhere a cell keeps it:
//! registers, stack, heap (`brk`) and a 4 MiB anonymous mapping (2 MiB
//! pages). It yields after every step and sleeps now and then, and prints a
//! digest every few steps: suspending and resuming it anywhere must not
//! change a byte of its output (spec `snapshots`, equivalence).
#![no_std]
#![no_main]

use rhizo_cells::{check, nr, print, sleep_ns, syscall, yield_now};

const STEPS: u64 = 40;
const BIG: usize = 4 << 20;

fn hex(mut v: u64, out: &mut [u8; 16]) {
    for i in (0..16).rev() {
        out[i] = b"0123456789abcdef"[(v & 15) as usize];
        v >>= 4;
    }
}

fn main() -> i32 {
    let heap = syscall(nr::BRK, [0; 6]) as usize;
    check(
        syscall(nr::BRK, [heap + 8192, 0, 0, 0, 0, 0]) as usize == heap + 8192,
        "brk",
    );
    let big = syscall(nr::MMAP, [0, BIG, 3, 0x22, usize::MAX, 0]);
    check(big > 0, "mmap");
    let (heap, big) = (heap as *mut u64, big as *mut u64);
    let mut stack = [0u64; 64];
    let mut digest: u64 = 0xcbf2_9ce4_8422_2325;
    for step in 0..STEPS {
        for i in 0..64 {
            // SAFETY: the heap has 1024 words and the mapping BIG / 8; every
            // index stays inside them.
            unsafe {
                let h = heap.add((step as usize * 64 + i) % 1024);
                let b = big.add((step as usize * 4099 + i * 977) % (BIG / 8));
                let v = digest ^ (step << 32 | i as u64);
                h.write_volatile(h.read_volatile().wrapping_add(v));
                b.write_volatile(b.read_volatile() ^ v.rotate_left(i as u32));
                stack[i] = stack[i].wrapping_mul(31) ^ h.read_volatile() ^ b.read_volatile();
                digest = (digest ^ stack[i]).wrapping_mul(0x100_0000_01b3);
            }
        }
        if step % 8 == 7 {
            let mut text = *b"step 00 digest 0123456789abcdef\n";
            text[5] = b'0' + (step / 10) as u8;
            text[6] = b'0' + (step % 10) as u8;
            let mut h = [0u8; 16];
            hex(digest, &mut h);
            text[15..31].copy_from_slice(&h);
            print(&text);
        }
        if step % 10 == 5 {
            sleep_ns(1_000_000);
        } else {
            yield_now();
        }
    }
    0
}

rhizo_cells::entry!(main);
