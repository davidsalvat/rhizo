//! Writes a value derived from its tag (argument 1) at a fixed address,
//! yields several times, and checks the value is still its own: cells with
//! the same layout must not share memory.
#![no_std]
#![no_main]

use rhizo_cells::{arg, check, print, yield_now};

static mut SLOT: u64 = 0;

fn main() -> i32 {
    let tag = arg(1).unwrap_or(b"0");
    let value = tag
        .iter()
        .fold(0x5eed_u64, |v, &b| v.wrapping_mul(31) + b as u64);
    // SAFETY: single-threaded program; volatile so the value is really in
    // memory across the yields.
    unsafe { core::ptr::addr_of_mut!(SLOT).write_volatile(value) };
    for _ in 0..5 {
        yield_now();
        // SAFETY: as above.
        let seen = unsafe { core::ptr::addr_of!(SLOT).read_volatile() };
        check(seen == value, "another cell's value");
    }
    print(b"own value ok\n");
    0
}

rhizo_cells::entry!(main);
