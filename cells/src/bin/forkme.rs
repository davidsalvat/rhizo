//! Fills 64 pages with a pattern, prints `ready` and yields: the host forks
//! it there. Every copy (parent and children) then checks the shared pages
//! are intact, writes into page 1 through a system call (`getrandom`) and its
//! identifier into every page from code, yields, and checks it sees only its
//! own writes (spec `fork`, copy-on-write isolation).
#![no_std]
#![no_main]

use rhizo_cells::{check, nr, print, syscall, yield_now};

const PAGES: usize = 64;

fn main() -> i32 {
    let p = syscall(nr::MMAP, [0, PAGES * 4096, 3, 0x22, usize::MAX, 0]);
    check(p > 0, "mmap");
    let words = p as *mut u64;
    let word = |page: usize, i: usize| {
        // SAFETY: page < PAGES and i < 512 stay inside the mapping.
        unsafe { words.add(page * 512 + i) }
    };
    for page in 0..PAGES {
        for i in 0..512 {
            // SAFETY: as above.
            unsafe { word(page, i).write_volatile((page * 512 + i) as u64) };
        }
    }
    print(b"ready\n");
    yield_now();

    for page in 0..PAGES {
        // SAFETY: as above.
        let v = unsafe { word(page, 100).read_volatile() };
        check(
            v == (page * 512 + 100) as u64,
            "a shared page changed under us",
        );
    }
    check(
        syscall(nr::GETRANDOM, [word(1, 0) as usize, 64, 0, 0, 0, 0]) == 64,
        "getrandom",
    );
    let me = syscall(nr::GETPID, [0; 6]) as u64;
    for page in 0..PAGES {
        // SAFETY: as above.
        unsafe { word(page, 0).write_volatile(me) };
    }
    for _ in 0..3 {
        yield_now();
    }
    for page in 0..PAGES {
        // SAFETY: as above.
        let (own, kept) = unsafe {
            (
                word(page, 0).read_volatile(),
                word(page, 100).read_volatile(),
            )
        };
        check(own == me, "another cell's write");
        check(kept == (page * 512 + 100) as u64, "lost a word");
    }
    print(b"own ok\n");
    0
}

rhizo_cells::entry!(main);
