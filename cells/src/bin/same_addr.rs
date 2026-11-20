//! Checks that its static starts with the value in the executable, writes a
//! value derived from its tag (argument 1) there, yields several times, and
//! checks the value is still its own: cells with the same layout must not
//! share memory, and a cell started after another of the same executable
//! never sees that one's writes.
#![no_std]
#![no_main]

use rhizo_cells::{arg, check, print, yield_now};

/// Not zero, so it lives in the data the executable carries.
const INITIAL: u64 = 0x1dea_5eed;
static mut SLOT: u64 = INITIAL;

fn main() -> i32 {
    // SAFETY: single-threaded program; volatile so the value is really read
    // from memory.
    let start = unsafe { core::ptr::addr_of!(SLOT).read_volatile() };
    check(start == INITIAL, "a static that is not fresh");
    let tag = arg(1).unwrap_or(b"0");
    let value = tag
        .iter()
        .fold(0x5eed_u64, |v, &b| v.wrapping_mul(31) + b as u64);
    // SAFETY: as above; volatile so the value is really in memory across
    // the yields.
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
