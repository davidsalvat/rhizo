//! What a copy of a cell can tell (design D22 of `add-cell-hierarchy`):
//!
//! - no argument: marks a page `MADV_WIPEONFORK` and writes 1 in it, reads
//!   its copy generation (`/run/rhizo/generation`) and `getpid`, prints
//!   `ready` and sleeps 20 ms (where a host forks it, freezes it or
//!   suspends it); then prints `page P generation G0 G1 pid same|changed`:
//!   the word in the page, the generation before and after, and whether
//!   `getpid` changed;
//! - `wait`: prints `start` and sleeps 20 ms first (where a host freezes
//!   it, so that its copies read the generation only once they are
//!   copies), then as without argument;
//! - `connect PORT ADDR`: connects to ADDR:PORT through the network card,
//!   prints `connected` and sleeps 10 s (where a host freezes it).
#![no_std]
#![no_main]

use rhizo_cells::{arg, check, loopback, nr, number, print, sleep_ns, syscall, tcp_socket};

const MADV_WIPEONFORK: usize = 18;

/// The cell's copy generation (`u64::MAX` where there is none, as on Linux).
fn generation() -> u64 {
    let path = b"/run/rhizo/generation\0";
    let fd = syscall(
        nr::OPENAT,
        [-100isize as usize, path.as_ptr() as usize, 0, 0, 0, 0],
    );
    if fd < 0 {
        return u64::MAX;
    }
    let mut word = [0u8; 8];
    let n = syscall(
        nr::READ,
        [fd as usize, word.as_mut_ptr() as usize, 8, 0, 0, 0],
    );
    check(n == 8, "read the generation");
    syscall(nr::CLOSE, [fd as usize, 0, 0, 0, 0, 0]);
    u64::from_le_bytes(word)
}

/// Prints `n` in decimal.
fn print_number(mut n: u64) {
    let mut digits = [0u8; 20];
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    print(&digits[at..]);
}

fn wipe() -> i32 {
    let page = syscall(nr::MMAP, [0, 4096, 3, 0x22, usize::MAX, 0]);
    check(page > 0, "mmap");
    let r = syscall(nr::MADVISE, [page as usize, 4096, MADV_WIPEONFORK, 0, 0, 0]);
    check(r == 0, "madvise");
    let word = page as *mut u64;
    // SAFETY: `word` is the start of the page just mapped, writable.
    unsafe { word.write_volatile(1) };
    let (before, pid) = (generation(), syscall(nr::GETPID, [0; 6]));
    print(b"ready\n");
    sleep_ns(20_000_000);
    // SAFETY: as above.
    let value = unsafe { word.read_volatile() };
    print(b"page ");
    print_number(value);
    print(b" generation ");
    print_number(before);
    print(b" ");
    print_number(generation());
    let same = syscall(nr::GETPID, [0; 6]) == pid;
    print(if same {
        b" pid same\n"
    } else {
        b" pid changed\n"
    });
    0
}

fn connect(port: u16, ip: [u8; 4]) -> i32 {
    let c = tcp_socket(0);
    check(c >= 0, "socket");
    let mut addr = loopback(port);
    addr[4..8].copy_from_slice(&ip);
    let r = syscall(
        nr::CONNECT,
        [c as usize, addr.as_ptr() as usize, 16, 0, 0, 0],
    );
    check(r == 0, "connect");
    print(b"connected\n");
    sleep_ns(10_000_000_000);
    0
}

fn main() -> i32 {
    match arg(1) {
        Some(b"connect") => {
            let mut ip = [0u8; 4];
            let text = arg(3).unwrap_or(b"");
            for (o, part) in ip.iter_mut().zip(text.split(|&b| b == b'.')) {
                *o = number(part) as u8;
            }
            connect(arg(2).map_or(0, number) as u16, ip)
        }
        Some(b"wait") => {
            print(b"start\n");
            sleep_ns(20_000_000);
            wipe()
        }
        _ => wipe(),
    }
}

rhizo_cells::entry!(main);
