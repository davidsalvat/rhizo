//! What a cell shares with others (design D21 of `add-cell-hierarchy`,
//! content sharing bounded by the domain):
//!
//! - `map PATH PAGES`: maps the first PAGES pages of file PATH read-only,
//!   reads a byte of each, prints `at 0xADDR` (where it mapped them) and
//!   sleeps until the host wakes it; then maps them again, and so on until
//!   it is ended;
//! - `write MIB`: maps MIB MiB and writes page `i`'s first word with
//!   `i + 1`, prints `ready` and sleeps until the host wakes it (or at once
//!   in a copy of its template); then checks every page still holds what
//!   it wrote, writes `i + 1000` into each, prints `written`, sleeps until
//!   woken again, and exits.
//!
//! Never run on Linux.
#![no_std]
#![no_main]

use rhizo_cells::{arg, check, nr, number, print, sleep_ns, syscall};

const AT_FDCWD: usize = -100isize as usize;
const PROT_READ: usize = 1;
const PROT_RW: usize = 3;
const MAP_PRIVATE: usize = 2;
const MAP_PRIVATE_ANONYMOUS: usize = 0x22;
const PAGE: usize = 4096;

/// Sleeps until the host wakes it.
fn until_woken() {
    sleep_ns(1000 * 1_000_000_000);
}

fn print_hex(mut n: usize) {
    let mut digits = [0u8; 16];
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b"0123456789abcdef"[n % 16];
        n /= 16;
        if n == 0 {
            break;
        }
    }
    print(b"0x");
    print(&digits[at..]);
}

fn map(path: &[u8], pages: usize) {
    let mut name = [0u8; 64];
    check(path.len() < name.len(), "path length");
    name[..path.len()].copy_from_slice(path);
    let fd = syscall(nr::OPENAT, [AT_FDCWD, name.as_ptr() as usize, 0, 0, 0, 0]);
    check(fd >= 0, "open");
    loop {
        let at = syscall(
            nr::MMAP,
            [0, pages * PAGE, PROT_READ, MAP_PRIVATE, fd as usize, 0],
        );
        check(at > 0, "mmap");
        for page in 0..pages {
            // SAFETY: inside the readable mapping just made.
            unsafe { ((at as usize + page * PAGE) as *const u8).read_volatile() };
        }
        print(b"at ");
        print_hex(at as usize);
        print(b"\n");
        until_woken();
    }
}

fn write(mib: usize) {
    let size = mib << 20;
    let p = syscall(
        nr::MMAP,
        [0, size, PROT_RW, MAP_PRIVATE_ANONYMOUS, usize::MAX, 0],
    );
    check(p > 0, "mmap");
    let word = |page: usize| (p as usize + page * PAGE) as *mut usize;
    for page in 0..size / PAGE {
        // SAFETY: inside the writable mapping just made.
        unsafe { word(page).write_volatile(page + 1) };
    }
    print(b"ready\n");
    until_woken();
    for page in 0..size / PAGE {
        // SAFETY: as above.
        check(unsafe { word(page).read_volatile() } == page + 1, "kept");
        // SAFETY: as above.
        unsafe { word(page).write_volatile(page + 1000) };
    }
    print(b"written\n");
    until_woken();
}

fn main() -> i32 {
    match arg(1) {
        Some(b"map") => map(arg(2).unwrap_or(b""), arg(3).map_or(1, number)),
        Some(b"write") => write(arg(2).map_or(1, number)),
        _ => check(false, "mode"),
    }
    0
}

rhizo_cells::entry!(main);
