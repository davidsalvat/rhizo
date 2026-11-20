//! Hostile: hands the kernel pointers into kernel memory, non-canonical
//! addresses and the last page of the lower half; every call must answer
//! `EFAULT`. Then asks `mprotect` and `munmap` for kernel ranges (refused),
//! for huge empty ranges (answered at once), and on x86_64 a kernel thread
//! pointer (`EPERM`). The same answers as on Linux.
#![no_std]
#![no_main]

use rhizo_cells::{check, nr, print, syscall};

#[cfg(target_arch = "aarch64")]
mod call {
    pub const OPENAT: usize = 56;
    pub const MKDIRAT: usize = 34;
    pub const FSTAT: usize = 80;
    pub const GETCWD: usize = 17;
    pub const RT_SIGPROCMASK: usize = 135;
    pub const MPROTECT: usize = 226;
}

#[cfg(target_arch = "x86_64")]
mod call {
    pub const OPENAT: usize = 257;
    pub const MKDIRAT: usize = 258;
    pub const FSTAT: usize = 5;
    pub const GETCWD: usize = 79;
    pub const RT_SIGPROCMASK: usize = 14;
    pub const MPROTECT: usize = 10;
    pub const ARCH_PRCTL: usize = 158;
}

const EPERM: isize = -1;
const ENOMEM: isize = -12;
const EFAULT: isize = -14;
const EINVAL: isize = -22;
const AT_FDCWD: usize = -100isize as usize;

/// Kernel data (direct map), kernel code, non-canonical, last lower page.
const BAD: [usize; 5] = [
    0xffff_8000_0000_0000,
    0xffff_ffff_8000_0000,
    0x8000_0000_0000_0000,
    0x0000_8000_0000_0000,
    0x0000_7fff_ffff_f000,
];

fn main() -> i32 {
    for p in BAD {
        for (number, args) in [
            (nr::WRITE, [1, p, 16, 0, 0, 0]),
            (nr::UNAME, [p, 0, 0, 0, 0, 0]),
            (nr::CLOCK_GETTIME, [1, p, 0, 0, 0, 0]),
            (nr::GETRANDOM, [p, 16, 0, 0, 0, 0]),
            (nr::NANOSLEEP, [p, 0, 0, 0, 0, 0]),
            (call::OPENAT, [AT_FDCWD, p, 0, 0, 0, 0]),
            (call::MKDIRAT, [AT_FDCWD, p, 0o755, 0, 0, 0]),
            (call::FSTAT, [1, p, 0, 0, 0, 0]),
            (call::GETCWD, [p, 4096, 0, 0, 0, 0]),
            (call::RT_SIGPROCMASK, [0, 0, p, 8, 0, 0]),
        ] {
            check(syscall(number, args) == EFAULT, "efault");
        }
    }
    print(b"efault ok\n");

    let (kernel, len) = (BAD[0], 2 << 20);
    for prot in [1, 3, 5] {
        check(
            syscall(call::MPROTECT, [kernel, len, prot, 0, 0, 0]) == ENOMEM,
            "mprotect kernel",
        );
    }
    check(
        syscall(nr::MUNMAP, [kernel, len, 0, 0, 0, 0]) == EINVAL,
        "munmap kernel",
    );
    print(b"kernel ranges ok\n");

    // Terabytes of nothing, below where Linux puts its own mappings.
    let (base, huge) = (0x4000_0000_0000, 0x3000_0000_0000);
    check(
        syscall(call::MPROTECT, [base, huge, 1, 0, 0, 0]) == ENOMEM,
        "mprotect huge",
    );
    check(
        syscall(nr::MUNMAP, [base, huge, 0, 0, 0, 0]) == 0,
        "munmap huge",
    );
    print(b"huge ranges ok\n");

    #[cfg(target_arch = "x86_64")]
    for p in BAD {
        const ARCH_SET_FS: usize = 0x1002;
        check(
            syscall(call::ARCH_PRCTL, [ARCH_SET_FS, p, 0, 0, 0, 0]) == EPERM,
            "arch_prctl",
        );
    }
    #[cfg(target_arch = "aarch64")]
    let _ = EPERM;
    print(b"thread pointer ok\n");
    0
}

rhizo_cells::entry!(main);
