//! Spins forever without a system call: only preemption can stop it.
#![no_std]
#![no_main]

fn main() -> i32 {
    let mut n: u64 = 0;
    loop {
        // SAFETY: a volatile write to a local keeps the loop from being
        // optimized away.
        unsafe { core::ptr::write_volatile(&mut n, n.wrapping_add(1)) };
    }
}

rhizo_cells::entry!(main);
