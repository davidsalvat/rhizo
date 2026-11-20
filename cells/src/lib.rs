//! A minimal runtime for Rhizo's test cells: raw Linux system calls and the
//! `_start` entry. The programs are `no_std` static executables, so the same
//! binary runs on Rhizo and on Linux (the differential check).

#![no_std]

/// Linux system-call numbers for the target architecture.
#[cfg(target_arch = "aarch64")]
pub mod nr {
    pub const WRITE: usize = 64;
    pub const EXIT_GROUP: usize = 94;
    pub const UNAME: usize = 160;
    pub const BRK: usize = 214;
    pub const MUNMAP: usize = 215;
    pub const MMAP: usize = 222;
    pub const CLOCK_GETTIME: usize = 113;
    pub const GETRANDOM: usize = 278;
    pub const GETPID: usize = 172;
    pub const SCHED_YIELD: usize = 124;
    pub const NANOSLEEP: usize = 101;
    pub const OPENAT: usize = 56;
    pub const CLOSE: usize = 57;
    pub const READ: usize = 63;
    pub const PREAD64: usize = 67;
    pub const PIPE2: usize = 59;
    pub const FCNTL: usize = 25;
    pub const SOCKET: usize = 198;
    pub const BIND: usize = 200;
    pub const LISTEN: usize = 201;
    pub const CONNECT: usize = 203;
    pub const GETSOCKNAME: usize = 204;
    pub const SENDTO: usize = 206;
    pub const SETSOCKOPT: usize = 208;
    pub const ACCEPT4: usize = 242;
    pub const CLONE: usize = 220;
    pub const EXIT: usize = 93;
    pub const FUTEX: usize = 98;
    pub const GETTID: usize = 178;
}

/// Linux system-call numbers for the target architecture.
#[cfg(target_arch = "x86_64")]
pub mod nr {
    pub const WRITE: usize = 1;
    pub const EXIT_GROUP: usize = 231;
    pub const UNAME: usize = 63;
    pub const BRK: usize = 12;
    pub const MUNMAP: usize = 11;
    pub const MMAP: usize = 9;
    pub const CLOCK_GETTIME: usize = 228;
    pub const GETRANDOM: usize = 318;
    pub const GETPID: usize = 39;
    pub const SCHED_YIELD: usize = 24;
    pub const NANOSLEEP: usize = 35;
    pub const OPENAT: usize = 257;
    pub const CLOSE: usize = 3;
    pub const READ: usize = 0;
    pub const PREAD64: usize = 17;
    pub const PIPE2: usize = 293;
    pub const FCNTL: usize = 72;
    pub const SOCKET: usize = 41;
    pub const BIND: usize = 49;
    pub const LISTEN: usize = 50;
    pub const CONNECT: usize = 42;
    pub const SENDTO: usize = 44;
    pub const GETSOCKNAME: usize = 51;
    pub const SETSOCKOPT: usize = 54;
    pub const ACCEPT4: usize = 288;
    pub const CLONE: usize = 56;
    pub const EXIT: usize = 60;
    pub const FUTEX: usize = 202;
    pub const GETTID: usize = 186;
    pub const ARCH_PRCTL: usize = 158;
}

/// Raw system call with up to six arguments; returns the kernel's value
/// (negative `errno` on failure).
#[inline(always)]
pub fn syscall(nr: usize, a: [usize; 6]) -> isize {
    let ret: isize;
    // SAFETY: a Linux system call only reads and writes the memory its
    // arguments describe; the callers pass valid or deliberately invalid
    // (but harmless) pointers.
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") nr,
            inlateout("x0") a[0] => ret,
            in("x1") a[1], in("x2") a[2], in("x3") a[3], in("x4") a[4], in("x5") a[5],
            options(nostack)
        );
    }
    // SAFETY: as above.
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") nr as isize => ret,
            in("rdi") a[0], in("rsi") a[1], in("rdx") a[2],
            in("r10") a[3], in("r8") a[4], in("r9") a[5],
            out("rcx") _, out("r11") _,
            options(nostack)
        );
    }
    ret
}

/// Writes `bytes` to standard output.
pub fn print(bytes: &[u8]) {
    syscall(
        nr::WRITE,
        [1, bytes.as_ptr() as usize, bytes.len(), 0, 0, 0],
    );
}

/// Ends the program with `status`.
pub fn exit(status: i32) -> ! {
    syscall(nr::EXIT_GROUP, [status as usize, 0, 0, 0, 0, 0]);
    loop {
        core::hint::spin_loop();
    }
}

/// Checks a condition; on failure prints `FAIL <what>` and exits with 1.
pub fn check(ok: bool, what: &str) {
    if !ok {
        print(b"FAIL ");
        print(what.as_bytes());
        print(b"\n");
        exit(1);
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    print(b"panic\n");
    exit(101)
}

/// Gives up the CPU to the next runnable cell.
pub fn yield_now() {
    syscall(nr::SCHED_YIELD, [0; 6]);
}

/// Sleeps for `ns` nanoseconds.
pub fn sleep_ns(ns: u64) {
    let ts = [(ns / 1_000_000_000) as i64, (ns % 1_000_000_000) as i64];
    syscall(nr::NANOSLEEP, [ts.as_ptr() as usize, 0, 0, 0, 0, 0]);
}

/// Decimal number in `bytes` (0 if none).
pub fn number(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .fold(0, |n, d| n * 10 + (d - b'0') as usize)
}

/// A `sockaddr_in` for 127.0.0.1:`port`.
pub fn loopback(port: u16) -> [u8; 16] {
    let mut a = [0u8; 16];
    a[0] = 2; // AF_INET
    a[2..4].copy_from_slice(&port.to_be_bytes());
    a[4..8].copy_from_slice(&[127, 0, 0, 1]);
    a
}

/// A TCP socket (`flags`: `SOCK_NONBLOCK` and so on); negative on error.
pub fn tcp_socket(flags: usize) -> isize {
    syscall(nr::SOCKET, [2, 1 | flags, 0, 0, 0, 0])
}

/// Binds `fd` to 127.0.0.1:`port` (`SO_REUSEADDR` first) and listens.
pub fn tcp_listen(fd: usize, port: u16) -> isize {
    let one = 1i32;
    syscall(
        nr::SETSOCKOPT,
        [fd, 1, 2, &one as *const i32 as usize, 4, 0],
    );
    let addr = loopback(port);
    let r = syscall(nr::BIND, [fd, addr.as_ptr() as usize, 16, 0, 0, 0]);
    if r < 0 {
        return r;
    }
    syscall(nr::LISTEN, [fd, 16, 0, 0, 0, 0])
}

/// Connects `fd` to 127.0.0.1:`port`.
pub fn tcp_connect(fd: usize, port: u16) -> isize {
    let addr = loopback(port);
    syscall(nr::CONNECT, [fd, addr.as_ptr() as usize, 16, 0, 0, 0])
}

/// The port `fd` is bound to.
pub fn local_port(fd: usize) -> u16 {
    let mut addr = [0u8; 16];
    let mut len = 16u32;
    syscall(
        nr::GETSOCKNAME,
        [
            fd,
            addr.as_mut_ptr() as usize,
            &mut len as *mut u32 as usize,
            0,
            0,
            0,
        ],
    );
    u16::from_be_bytes([addr[2], addr[3]])
}

/// Reads exactly `buf.len()` bytes from `fd` (fewer at the end of the
/// stream); returns how many, or the error.
pub fn read_full(fd: usize, buf: &mut [u8]) -> isize {
    let mut done = 0;
    while done < buf.len() {
        let n = syscall(
            nr::READ,
            [
                fd,
                buf[done..].as_mut_ptr() as usize,
                buf.len() - done,
                0,
                0,
                0,
            ],
        );
        if n <= 0 {
            return if done > 0 { done as isize } else { n };
        }
        done += n as usize;
    }
    done as isize
}

/// Writes all of `buf` to `fd`; returns how many, or the error.
pub fn write_all(fd: usize, buf: &[u8]) -> isize {
    let mut done = 0;
    while done < buf.len() {
        let n = syscall(
            nr::WRITE,
            [fd, buf[done..].as_ptr() as usize, buf.len() - done, 0, 0, 0],
        );
        if n <= 0 {
            return n;
        }
        done += n as usize;
    }
    done as isize
}

/// Address of `argc` on the initial stack, saved by `_start`.
static mut STACK: usize = 0;

/// Command-line argument `n` (0 is the program name).
pub fn arg(n: usize) -> Option<&'static [u8]> {
    // SAFETY: `_start` stores the initial stack pointer before `main` runs;
    // the kernel (or Linux) put argc, then argc NUL-terminated strings there,
    // and nothing writes to them.
    unsafe {
        let sp = core::ptr::addr_of!(STACK).read() as *const usize;
        if sp.is_null() || n >= *sp {
            return None;
        }
        let s = *sp.add(1 + n) as *const u8;
        let mut len = 0;
        while *s.add(len) != 0 {
            len += 1;
        }
        Some(core::slice::from_raw_parts(s, len))
    }
}

#[doc(hidden)]
pub fn start(sp: usize, main: fn() -> i32) -> ! {
    // SAFETY: single-threaded program; written once, before any reader.
    unsafe { core::ptr::addr_of_mut!(STACK).write(sp) };
    exit(main())
}

/// Defines `_start`, which calls `$main` and exits with its return value.
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        #[unsafe(no_mangle)]
        extern "C" fn rhizo_cell_main(sp: usize) -> ! {
            $crate::start(sp, $main)
        }

        #[cfg(target_arch = "aarch64")]
        core::arch::global_asm!(
            ".globl _start",
            "_start:",
            "    mov x29, xzr",
            "    mov x30, xzr",
            "    mov x0, sp",
            "    bl rhizo_cell_main",
        );

        #[cfg(target_arch = "x86_64")]
        core::arch::global_asm!(
            ".globl _start",
            "_start:",
            "    xor ebp, ebp",
            "    mov rdi, rsp",
            "    and rsp, -16",
            "    call rhizo_cell_main",
        );
    };
}

/// Monotonic time in nanoseconds.
pub fn now_ns() -> u64 {
    let mut ts = [0u64; 2];
    syscall(nr::CLOCK_GETTIME, [1, ts.as_mut_ptr() as usize, 0, 0, 0, 0]);
    ts[0] * 1_000_000_000 + ts[1]
}

/// The flags glibc's `pthread_create` passes to `clone`: `CLONE_VM`, `FS`,
/// `FILES`, `SIGHAND`, `THREAD`, `SYSVSEM`, `SETTLS`, `PARENT_SETTID` and
/// `CHILD_CLEARTID`.
pub const THREAD_FLAGS: usize = 0x3d_0f00;

/// Starts a thread running `f(arg)` on the stack whose top is `stack_top`
/// (16-byte aligned), with thread pointer `tls`. The thread's identifier is
/// written to `*tid`, which the kernel clears and wakes when the thread
/// exits (see [`join`]); the thread exits with `f`'s result. Returns the
/// identifier, or a negative `errno`.
pub fn spawn_thread(
    f: extern "C" fn(usize) -> usize,
    arg: usize,
    stack_top: usize,
    tls: usize,
    tid: &core::sync::atomic::AtomicU32,
) -> isize {
    let tid = tid.as_ptr() as usize;
    let ret: isize;
    // SAFETY: `clone` with `CLONE_VM` starts a thread in this address space
    // on the given stack; the child (result 0) never returns into Rust code:
    // it calls `f(arg)` on its own stack and exits with its result. The
    // parent continues with the new identifier, its registers as declared.
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!(
            "svc #0",
            "cbnz x0, 2f",
            "mov x0, x10",
            "blr x9",
            "mov x8, #93",
            "svc #0",
            "2:",
            in("x8") nr::CLONE,
            inlateout("x0") THREAD_FLAGS => ret,
            in("x1") stack_top, in("x2") tid, in("x3") tls, in("x4") tid,
            in("x9") f, in("x10") arg,
            options(nostack)
        );
    }
    // SAFETY: as above.
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!(
            "syscall",
            "test rax, rax",
            "jnz 2f",
            "mov rdi, r13",
            "call r12",
            "mov edi, eax",
            "mov eax, 60",
            "syscall",
            "ud2",
            "2:",
            inlateout("rax") nr::CLONE as isize => ret,
            in("rdi") THREAD_FLAGS, in("rsi") stack_top, in("rdx") tid,
            in("r10") tid, in("r8") tls, in("r12") f, in("r13") arg,
            out("rcx") _, out("r11") _,
            options(nostack)
        );
    }
    ret
}

const FUTEX_PRIVATE: usize = 128;

/// `FUTEX_WAIT_BITSET` (private) on `word` while it holds `val`, with an
/// optional relative timeout in nanoseconds (`FUTEX_WAIT`, then); returns
/// the call's result.
pub fn futex_wait(
    word: &core::sync::atomic::AtomicU32,
    val: u32,
    timeout_ns: Option<u64>,
    bitset: u32,
) -> isize {
    let ts = timeout_ns.map(|ns| [(ns / 1_000_000_000) as i64, (ns % 1_000_000_000) as i64]);
    let (op, ts_ptr) = match &ts {
        Some(ts) => (FUTEX_PRIVATE, ts.as_ptr() as usize), // FUTEX_WAIT
        None => (9 | FUTEX_PRIVATE, 0),                    // FUTEX_WAIT_BITSET
    };
    syscall(
        nr::FUTEX,
        [
            word.as_ptr() as usize,
            op,
            val as usize,
            ts_ptr,
            0,
            bitset as usize,
        ],
    )
}

/// `FUTEX_WAKE_BITSET` (private): wakes up to `n` waiters on `word` whose
/// bitset intersects `bitset`; returns how many it woke.
pub fn futex_wake(word: &core::sync::atomic::AtomicU32, n: u32, bitset: u32) -> isize {
    syscall(
        nr::FUTEX,
        [
            word.as_ptr() as usize,
            10 | FUTEX_PRIVATE,
            n as usize,
            0,
            0,
            bitset as usize,
        ],
    )
}

/// Waits until the thread whose identifier is in `tid` has exited (the
/// kernel clears the word and wakes it), as `pthread_join` does: with a
/// shared futex wait, because Linux wakes the word with a shared wake.
pub fn join(tid: &core::sync::atomic::AtomicU32) {
    use core::sync::atomic::Ordering;
    loop {
        let t = tid.load(Ordering::Acquire);
        if t == 0 {
            return;
        }
        // FUTEX_WAIT_BITSET, not private.
        syscall(
            nr::FUTEX,
            [
                tid.as_ptr() as usize,
                9,
                t as usize,
                0,
                0,
                u32::MAX as usize,
            ],
        );
    }
}
