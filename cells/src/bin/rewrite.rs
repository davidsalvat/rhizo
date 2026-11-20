//! Hostile after a fork: prints `ready` and yields (the host forks it
//! there, so its read-only pages are shared), then makes its page of
//! constants writable with `mprotect`, writes its identifier there, yields,
//! and checks it still reads its own identifier, not another copy's: the
//! write went to a private page (spec `isolation`). Prints `own ok`.
#![no_std]
#![no_main]

use rhizo_cells::{check, nr, print, syscall, yield_now};

#[cfg(target_arch = "aarch64")]
const MPROTECT: usize = 226;
#[cfg(target_arch = "x86_64")]
const MPROTECT: usize = 10;

/// A page of constants of its own (read-only, shared after a fork).
#[repr(C, align(4096))]
struct Page([u64; 512]);

static CONSTANTS: Page = Page([7; 512]);

fn main() -> i32 {
    let page = core::ptr::addr_of!(CONSTANTS) as *mut u64;
    print(b"ready\n");
    yield_now();
    check(
        syscall(MPROTECT, [page as usize, 4096, 3, 0, 0, 0]) == 0,
        "mprotect",
    );
    let me = syscall(nr::GETPID, [0; 6]) as u64;
    // SAFETY: the page is writable now (above); volatile accesses, so the
    // compiler assumes nothing about the constant's value.
    unsafe { page.write_volatile(me) };
    for _ in 0..3 {
        yield_now();
    }
    // SAFETY: as above.
    let (mine, next) = unsafe { (page.read_volatile(), page.add(1).read_volatile()) };
    check(mine == me, "another copy's write");
    check(next == 7, "constants intact");
    print(b"own ok\n");
    0
}

rhizo_cells::entry!(main);
