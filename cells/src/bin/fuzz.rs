//! Hostile: makes 20000 system calls with numbers and arguments from a
//! fixed-seed generator (kernel and non-canonical addresses, huge lengths,
//! negative descriptors, wild flags), and checks every answer is a value or
//! a Linux `errno`. Only calls that would end it, put it to sleep or take
//! away its own stack are tamed. Prints `fuzz ok`. Never run on Linux.
#![no_std]
#![no_main]

use rhizo_cells::{check, exit, nr, syscall};

#[cfg(target_arch = "aarch64")]
mod call {
    pub const ENDS_OR_SLEEPS: &[usize] = &[93, 94, 101, 115, 72];
    pub const MEMORY: &[usize] = &[215, 226, 233]; // munmap, mprotect, madvise
    pub const DUP3: usize = 24;
}

#[cfg(target_arch = "x86_64")]
mod call {
    pub const ENDS_OR_SLEEPS: &[usize] = &[60, 231, 35, 230, 23, 270];
    pub const MEMORY: &[usize] = &[11, 10, 28]; // munmap, mprotect, madvise
    pub const DUP3: usize = 292;
}

/// Where the program keeps standard output for its last line.
const OUT: usize = 1023;
const SCRATCH_LEN: usize = 64 << 10;
const CALLS: usize = 20_000;

/// xorshift64*: the same sequence on every run and architecture.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// An argument a hostile program would pass.
    fn arg(&mut self, scratch: usize) -> usize {
        (match self.below(12) {
            0 => self.next(),
            1 => self.below(4097),
            2 => self.below(8),
            3 => (-(self.below(4097) as i64)) as u64,
            4 => 0xffff_8000_0000_0000 | self.next(),
            5 => 0x0000_8000_0000_0000 + self.below(0xffff_0000_0000_0000),
            6 => self.below(0x0000_8000_0000_0000),
            7 => scratch as u64 + self.below(SCRATCH_LEN as u64 + 64),
            8 => 1 << self.below(64),
            9 => [0, u64::MAX, 0x0000_7fff_ffff_f000, 0x4000_0000_0000][self.below(4) as usize],
            10 => scratch as u64,
            _ => (-100i64) as u64, // AT_FDCWD
        }) as usize
    }
}

fn main() -> i32 {
    // Standard output moves out of reach of the calls below.
    check(
        syscall(call::DUP3, [1, OUT, 0, 0, 0, 0]) == OUT as isize,
        "dup3",
    );
    for fd in 0..3 {
        syscall(nr::CLOSE, [fd, 0, 0, 0, 0, 0]);
    }
    // Memory the kernel may write into; the program never reads it.
    let scratch = syscall(nr::MMAP, [0, SCRATCH_LEN, 3, 0x22, usize::MAX, 0]);
    check(scratch > 0, "mmap");
    let scratch = scratch as usize;
    let mut rng = Rng(0x0005_eed0_fa11_ce11);
    for _ in 0..CALLS {
        let number = match rng.below(10) {
            0 => rng.next() as usize,
            1 => 1024 + rng.below(100_000) as usize,
            _ => rng.below(460) as usize,
        };
        if call::ENDS_OR_SLEEPS.contains(&number) {
            continue;
        }
        let mut a = [0usize; 6];
        for v in &mut a {
            *v = rng.arg(scratch);
            if *v as u32 == OUT as u32 {
                *v = OUT - 1;
            }
        }
        // Memory calls on the lower half stay within the scratch region.
        if call::MEMORY.contains(&number) && a[0] < 0x0000_8000_0000_0000 {
            a[0] = scratch;
            a[1] = a[1].min(SCRATCH_LEN);
        }
        let r = syscall(number, a);
        if r < -4095 {
            say(b"FAIL result is neither a value nor an errno\n");
            exit(1);
        }
    }
    say(b"fuzz ok\n");
    0
}

fn say(text: &[u8]) {
    syscall(
        nr::WRITE,
        [OUT, text.as_ptr() as usize, text.len(), 0, 0, 0],
    );
}

rhizo_cells::entry!(main);
