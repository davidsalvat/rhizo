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

/// Address of `argc` on the initial stack, saved by `_start`.
static mut STACK: *const usize = core::ptr::null();

/// Command-line argument `n` (0 is the program name).
pub fn arg(n: usize) -> Option<&'static [u8]> {
    // SAFETY: `_start` stores the initial stack pointer before `main` runs;
    // the kernel (or Linux) put argc, then argc NUL-terminated strings there,
    // and nothing writes to them.
    unsafe {
        let sp = core::ptr::addr_of!(STACK).read();
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
pub fn start(sp: *const usize, main: fn() -> i32) -> ! {
    // SAFETY: single-threaded program; written once, before any reader.
    unsafe { core::ptr::addr_of_mut!(STACK).write(sp) };
    exit(main())
}

/// Defines `_start`, which calls `$main` and exits with its return value.
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        #[unsafe(no_mangle)]
        extern "C" fn rhizo_cell_main(sp: *const usize) -> ! {
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
