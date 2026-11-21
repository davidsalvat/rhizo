//! A local agent of the laptop workload (spec `benchmarks`, design D1 of
//! `add-reference-workloads`): `agent ADDR PORT`, with a shared folder in
//! `/shared` (seeded by the host) and its own work in `/work`.
//!
//! - Warm-up: fills a 1 MiB history, digests `/shared/notes.txt`, prints
//!   `ready` and sleeps; the host freezes it there as a template.
//! - First wake (a fork starts at once): opens its session, writing
//!   `/work/history.txt`, prints `open` and sleeps until its user writes.
//! - Every later wake is a user message, answered in one turn: an HTTP
//!   request to the model at ADDR:PORT, its answer appended to the history,
//!   and a small tool action (`/shared/notes.txt` read back and checked);
//!   it prints `turn` and sleeps again.
#![no_std]
#![no_main]

use rhizo_cells::{
    arg, check, loopback, nr, number, print, sleep_ns, syscall, tcp_socket, write_all,
};

const AT_FDCWD: usize = -100isize as usize;
const HISTORY: usize = 1 << 20;
const NOTES: usize = 64 << 10;
const FOREVER: u64 = 3_600_000_000_000;

static mut MEMORY: [u8; HISTORY] = [0; HISTORY];
static mut NOTES_BUF: [u8; NOTES] = [0; NOTES];

fn open(path: &[u8], flags: usize) -> usize {
    let fd = syscall(
        nr::OPENAT,
        [AT_FDCWD, path.as_ptr() as usize, flags, 0o644, 0, 0],
    );
    check(fd >= 0, "open");
    fd as usize
}

/// FNV-1a digest of the shared notes.
fn notes_digest() -> u64 {
    // SAFETY: single-threaded program; the only reference to NOTES_BUF.
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(NOTES_BUF) };
    let fd = open(b"/shared/notes.txt\0", 0);
    let n = syscall(nr::PREAD64, [fd, buf.as_mut_ptr() as usize, NOTES, 0, 0, 0]);
    check(n > 0, "read notes");
    syscall(nr::CLOSE, [fd, 0, 0, 0, 0, 0]);
    buf[..n as usize]
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
            (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
        })
}

/// A dotted IPv4 address.
fn ipv4(text: &[u8]) -> [u8; 4] {
    let mut out = [0u8; 4];
    for (o, part) in out.iter_mut().zip(text.split(|&b| b == b'.')) {
        *o = number(part) as u8;
    }
    out
}

/// One request to the model; returns the length of the answer's body,
/// which is left in `answer`.
fn ask(ip: [u8; 4], port: u16, answer: &mut [u8]) -> usize {
    let c = tcp_socket(0);
    check(c >= 0, "socket");
    let c = c as usize;
    let mut addr = loopback(port);
    addr[4..8].copy_from_slice(&ip);
    check(
        syscall(nr::CONNECT, [c, addr.as_ptr() as usize, 16, 0, 0, 0]) == 0,
        "connect to the model",
    );
    let body = b"{\"messages\":[{\"role\":\"user\",\"content\":\"next step\"}]}";
    let head = b"POST /v1/chat/completions HTTP/1.1\r\nHost: model\r\n\
                 Content-Type: application/json\r\nConnection: close\r\nContent-Length: ";
    let len = [
        b'0' + (body.len() / 10) as u8,
        b'0' + (body.len() % 10) as u8,
    ];
    for part in [&head[..], &len, b"\r\n\r\n", body] {
        check(write_all(c, part) == part.len() as isize, "send");
    }
    // The answer, until the model closes the connection.
    let mut got = 0;
    loop {
        let n = syscall(
            nr::READ,
            [
                c,
                answer[got..].as_mut_ptr() as usize,
                answer.len() - got,
                0,
                0,
                0,
            ],
        );
        check(n >= 0, "receive");
        if n == 0 {
            break;
        }
        got += n as usize;
        check(got < answer.len(), "answer too long");
    }
    syscall(nr::CLOSE, [c, 0, 0, 0, 0, 0]);
    check(answer.starts_with(b"HTTP/1.1 200"), "status");
    let start = answer[..got]
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4);
    check(start.is_some(), "headers");
    let start = start.unwrap_or(0);
    answer.copy_within(start..got, 0);
    got - start
}

fn main() -> i32 {
    let ip = ipv4(arg(1).unwrap_or(b"127.0.0.1"));
    let port = arg(2).map_or(0, number) as u16;
    // SAFETY: single-threaded program; the only reference to MEMORY.
    let memory = unsafe { &mut *core::ptr::addr_of_mut!(MEMORY) };
    for (i, b) in memory.iter_mut().enumerate() {
        *b = i as u8;
    }
    let digest = notes_digest();
    print(b"ready\n");
    sleep_ns(FOREVER);

    let history = open(b"/work/history.txt\0", 0x1 | 0x40 | 0x400); // O_WRONLY | O_CREAT | O_APPEND
    check(write_all(history, &memory[..4096]) == 4096, "open session");
    print(b"open\n");
    let mut answer = [0u8; 2048];
    let mut turn = 0;
    loop {
        sleep_ns(FOREVER);
        let n = ask(ip, port, &mut answer);
        // The tool: keep the answer, look at the shared notes again.
        check(
            write_all(history, &answer[..n]) == n as isize,
            "keep the answer",
        );
        memory[turn % HISTORY] = answer[0];
        check(notes_digest() == digest, "shared notes unchanged");
        turn += 1;
        print(b"turn\n");
    }
}

rhizo_cells::entry!(main);
