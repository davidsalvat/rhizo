//! Keeps a file open across scheduling steps (spec `files`): prints the
//! contents of `input.txt` if the cell was given one, writes `<tag>:` into
//! `diary.txt` (the tag is the first argument, `a` by default), prints
//! `ready` and yields; the host may fork, freeze or suspend it there. Then
//! each copy writes its identifier through the same descriptor, yields, and
//! checks the file holds exactly its own writes.
#![no_std]
#![no_main]

use rhizo_cells::{arg, check, nr, print, syscall, yield_now};

const AT_FDCWD: usize = -100isize as usize;
const O_RDWR_CREAT_TRUNC: usize = 0x2 | 0x40 | 0x200;

fn open(path: &[u8], flags: usize) -> isize {
    syscall(
        nr::OPENAT,
        [AT_FDCWD, path.as_ptr() as usize, flags, 0o644, 0, 0],
    )
}

fn write(fd: usize, data: &[u8]) -> isize {
    syscall(nr::WRITE, [fd, data.as_ptr() as usize, data.len(), 0, 0, 0])
}

/// Decimal digits of `n` into `out`; returns how many.
fn decimal(mut n: usize, out: &mut [u8; 20]) -> usize {
    let mut digits = [0u8; 20];
    let mut len = 0;
    loop {
        digits[len] = b'0' + (n % 10) as u8;
        len += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for i in 0..len {
        out[i] = digits[len - 1 - i];
    }
    len
}

fn main() -> i32 {
    let mut buf = [0u8; 128];
    let input = open(b"input.txt\0", 0);
    if input >= 0 {
        let n = syscall(
            nr::READ,
            [input as usize, buf.as_mut_ptr() as usize, 64, 0, 0, 0],
        );
        check(n >= 0, "read input");
        print(b"input ");
        print(&buf[..n as usize]);
        print(b"\n");
        syscall(nr::CLOSE, [input as usize, 0, 0, 0, 0, 0]);
    }
    let fd = open(b"diary.txt\0", O_RDWR_CREAT_TRUNC);
    check(fd >= 0, "open diary");
    let fd = fd as usize;
    let tag = arg(1).unwrap_or(b"a");
    check(
        write(fd, tag) == tag.len() as isize && write(fd, b":") == 1,
        "write tag",
    );
    print(b"ready\n");
    yield_now();

    let mut me = [0u8; 21];
    let len = decimal(
        syscall(nr::GETPID, [0; 6]) as usize,
        (&mut me[..20]).try_into().unwrap(),
    );
    me[len] = b'\n';
    let mine = &me[..=len];
    check(write(fd, mine) == mine.len() as isize, "write id");
    for _ in 0..3 {
        yield_now();
    }
    let n = syscall(
        nr::PREAD64,
        [fd, buf.as_mut_ptr() as usize, buf.len(), 0, 0, 0],
    );
    let expected_len = tag.len() + 1 + mine.len();
    check(n == expected_len as isize, "file length");
    check(
        &buf[..tag.len()] == tag && buf[tag.len()] == b':',
        "tag kept",
    );
    check(&buf[tag.len() + 1..expected_len] == mine, "own id only");
    // The descriptor's offset is at the end: nothing more to read.
    let more = syscall(nr::READ, [fd, buf.as_mut_ptr() as usize, 1, 0, 0, 0]);
    check(more == 0, "offset kept");
    print(b"own ok\n");
    0
}

rhizo_cells::entry!(main);
