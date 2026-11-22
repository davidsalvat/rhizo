//! The Linux system-call interface of Rhizo cells (spec `linux-abi`).
//!
//! [`Abi::handle`] decodes the call from a cell's saved registers with the
//! Linux convention of its architecture, runs it against the cell's address
//! space and the kernel [`Services`], and writes the result (or the negated
//! `errno`) back. Everything goes through traits, so it is tested on the host.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod exec;
mod files;
mod pages;
mod poll;
mod sockets;
mod threads;

pub use exec::{ExecError, exec};
pub use pages::{FilePages, PUBLIC, PageCache, Share, own_copy};

use core::fmt;
use rhizo_cell::memory::{self, Pager};
use rhizo_cell::quota::Quota;
use rhizo_cell::space::{self, Frames, TooManyRanges, WxRanges};
use rhizo_cell::threads::Wait;
use rhizo_cell::{Cell, FpState, MMAP_BASE, STACK_LIMIT, STACK_TOP, USER_END};
use rhizo_hal::{Console, PhysicalMemory, Tlb};
use rhizo_mm::paging::{EntryFormat, PageFlags, PageTable};
use rhizo_mm::{FRAME_SIZE, LARGE_PAGE_SIZE, align_up};
use rhizo_net::Ports;

/// Linux `errno` values used here.
pub mod errno {
    pub const EPERM: i64 = 1;
    pub const ENOENT: i64 = 2;
    pub const EBADF: i64 = 9;
    pub const EAGAIN: i64 = 11;
    pub const ENOMEM: i64 = 12;
    pub const EACCES: i64 = 13;
    pub const EFAULT: i64 = 14;
    pub const EINVAL: i64 = 22;
    pub const ESRCH: i64 = 3;
    pub const ENODEV: i64 = 19;
    pub const ENOTTY: i64 = 25;
    pub const ESPIPE: i64 = 29;
    pub const EPIPE: i64 = 32;
    pub const ENOSYS: i64 = 38;
    pub const ENOTSOCK: i64 = 88;
    pub const ENOPROTOOPT: i64 = 92;
    pub const EPROTONOSUPPORT: i64 = 93;
    pub const ESOCKTNOSUPPORT: i64 = 94;
    pub const EOPNOTSUPP: i64 = 95;
    pub const EAFNOSUPPORT: i64 = 97;
    pub const EADDRINUSE: i64 = 98;
    pub const ENOBUFS: i64 = 105;
    pub const ENOTCONN: i64 = 107;
    pub const EINPROGRESS: i64 = 115;
    pub const ETIMEDOUT: i64 = 110;
}
use errno::*;

/// Instruction set of the cell, which fixes the call numbers and registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    /// Number in `x8`, arguments in `x0`–`x5`, result in `x0`.
    Aarch64,
    /// Number in `rax`, arguments in `rdi`, `rsi`, `rdx`, `r10`, `r8`, `r9`,
    /// result in `rax`.
    X86_64,
}

/// The system calls Rhizo implements (design D6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sys {
    Write,
    Writev,
    Exit,
    ExitGroup,
    Brk,
    Mmap,
    Munmap,
    Getpid,
    Gettid,
    SetTidAddress,
    Uname,
    ClockGettime,
    Time,
    Getrandom,
    RtSigaction,
    RtSigprocmask,
    Ioctl,
    ArchPrctl,
    SchedYield,
    Nanosleep,
    ClockNanosleep,
    Mprotect,
    SetRobustList,
    Rseq,
    Prlimit64,
    Getrlimit,
    Readlink,
    Readlinkat,
    Ids,
    Getppid,
    Sigaltstack,
    Madvise,
    Futex,
    Fstat,
    Newfstatat,
    Stat,
    Lstat,
    Openat,
    Open,
    Close,
    Read,
    Pread,
    Pwrite,
    Lseek,
    Getdents64,
    Mkdirat,
    Mkdir,
    Unlinkat,
    Unlink,
    Rmdir,
    Renameat,
    Renameat2,
    Rename,
    Getcwd,
    Chdir,
    Ftruncate,
    Fsync,
    Dup,
    Dup2,
    Dup3,
    Fcntl,
    Faccessat,
    Faccessat2,
    Access,
    Select,
    Pselect6,
    Readv,
    Pipe2,
    Pipe,
    Eventfd2,
    Eventfd,
    Poll,
    Ppoll,
    EpollCreate1,
    EpollCreate,
    EpollCtl,
    EpollWait,
    EpollPwait,
    Socket,
    Socketpair,
    Bind,
    Listen,
    Accept,
    Accept4,
    Connect,
    Getsockname,
    Getpeername,
    Sendto,
    Recvfrom,
    Setsockopt,
    Getsockopt,
    Shutdown,
    Sendmsg,
    Recvmsg,
    Clone,
    Clone3,
    Sysinfo,
    Statx,
    SchedGetaffinity,
    Fadvise64,
    Gettimeofday,
    ClockGetres,
    Capget,
    Prctl,
    SchedGetparam,
    SchedGetscheduler,
    IoUringSetup,
}

impl Arch {
    fn decode(self, nr: u64) -> Option<Sys> {
        use Sys::*;
        Some(match (self, nr) {
            (Arch::Aarch64, 29) | (Arch::X86_64, 16) => Ioctl,
            (Arch::Aarch64, 64) | (Arch::X86_64, 1) => Write,
            (Arch::Aarch64, 66) | (Arch::X86_64, 20) => Writev,
            (Arch::Aarch64, 93) | (Arch::X86_64, 60) => Exit,
            (Arch::Aarch64, 94) | (Arch::X86_64, 231) => ExitGroup,
            (Arch::Aarch64, 96) | (Arch::X86_64, 218) => SetTidAddress,
            (Arch::Aarch64, 113) | (Arch::X86_64, 228) => ClockGettime,
            (Arch::Aarch64, 134) | (Arch::X86_64, 13) => RtSigaction,
            (Arch::Aarch64, 135) | (Arch::X86_64, 14) => RtSigprocmask,
            (Arch::Aarch64, 160) | (Arch::X86_64, 63) => Uname,
            (Arch::Aarch64, 172) | (Arch::X86_64, 39) => Getpid,
            (Arch::Aarch64, 178) | (Arch::X86_64, 186) => Gettid,
            (Arch::Aarch64, 214) | (Arch::X86_64, 12) => Brk,
            (Arch::Aarch64, 215) | (Arch::X86_64, 11) => Munmap,
            (Arch::Aarch64, 222) | (Arch::X86_64, 9) => Mmap,
            (Arch::Aarch64, 278) | (Arch::X86_64, 318) => Getrandom,
            (Arch::Aarch64, 124) | (Arch::X86_64, 24) => SchedYield,
            (Arch::Aarch64, 226) | (Arch::X86_64, 10) => Mprotect,
            (Arch::Aarch64, 99) | (Arch::X86_64, 273) => SetRobustList,
            (Arch::Aarch64, 293) | (Arch::X86_64, 334) => Rseq,
            (Arch::Aarch64, 261) | (Arch::X86_64, 302) => Prlimit64,
            (Arch::Aarch64, 163) | (Arch::X86_64, 97) => Getrlimit,
            (Arch::X86_64, 89) => Readlink,
            (Arch::Aarch64, 78) | (Arch::X86_64, 267) => Readlinkat,
            // getuid, geteuid, getgid, getegid
            (Arch::Aarch64, 174..=177) | (Arch::X86_64, 102 | 104 | 107 | 108) => Ids,
            (Arch::Aarch64, 173) | (Arch::X86_64, 110) => Getppid,
            (Arch::Aarch64, 132) | (Arch::X86_64, 131) => Sigaltstack,
            (Arch::Aarch64, 233) | (Arch::X86_64, 28) => Madvise,
            (Arch::Aarch64, 98) | (Arch::X86_64, 202) => Futex,
            (Arch::Aarch64, 80) | (Arch::X86_64, 5) => Fstat,
            (Arch::Aarch64, 79) | (Arch::X86_64, 262) => Newfstatat,
            (Arch::Aarch64, 101) | (Arch::X86_64, 35) => Nanosleep,
            (Arch::Aarch64, 115) | (Arch::X86_64, 230) => ClockNanosleep,
            (Arch::X86_64, 158) => ArchPrctl,
            (Arch::X86_64, 201) => Time,
            (Arch::X86_64, 4) => Stat,
            (Arch::X86_64, 6) => Lstat,
            (Arch::Aarch64, 56) | (Arch::X86_64, 257) => Openat,
            (Arch::X86_64, 2) => Open,
            (Arch::Aarch64, 57) | (Arch::X86_64, 3) => Close,
            (Arch::Aarch64, 63) | (Arch::X86_64, 0) => Read,
            (Arch::Aarch64, 67) | (Arch::X86_64, 17) => Pread,
            (Arch::Aarch64, 68) | (Arch::X86_64, 18) => Pwrite,
            (Arch::Aarch64, 62) | (Arch::X86_64, 8) => Lseek,
            (Arch::Aarch64, 61) | (Arch::X86_64, 217) => Getdents64,
            (Arch::Aarch64, 34) | (Arch::X86_64, 258) => Mkdirat,
            (Arch::X86_64, 83) => Mkdir,
            (Arch::Aarch64, 35) | (Arch::X86_64, 263) => Unlinkat,
            (Arch::X86_64, 87) => Unlink,
            (Arch::X86_64, 84) => Rmdir,
            (Arch::Aarch64, 38) | (Arch::X86_64, 264) => Renameat,
            (Arch::Aarch64, 276) | (Arch::X86_64, 316) => Renameat2,
            (Arch::X86_64, 82) => Rename,
            (Arch::Aarch64, 17) | (Arch::X86_64, 79) => Getcwd,
            (Arch::Aarch64, 49) | (Arch::X86_64, 80) => Chdir,
            (Arch::Aarch64, 46) | (Arch::X86_64, 77) => Ftruncate,
            // fsync, fdatasync
            (Arch::Aarch64, 82 | 83) | (Arch::X86_64, 74 | 75) => Fsync,
            (Arch::Aarch64, 23) | (Arch::X86_64, 32) => Dup,
            (Arch::X86_64, 33) => Dup2,
            (Arch::Aarch64, 24) | (Arch::X86_64, 292) => Dup3,
            (Arch::Aarch64, 25) | (Arch::X86_64, 72) => Fcntl,
            (Arch::Aarch64, 48) | (Arch::X86_64, 269) => Faccessat,
            (Arch::Aarch64, 439) | (Arch::X86_64, 439) => Faccessat2,
            (Arch::X86_64, 21) => Access,
            (Arch::X86_64, 23) => Select,
            (Arch::Aarch64, 72) | (Arch::X86_64, 270) => Pselect6,
            (Arch::Aarch64, 65) | (Arch::X86_64, 19) => Readv,
            (Arch::Aarch64, 59) | (Arch::X86_64, 293) => Pipe2,
            (Arch::X86_64, 22) => Pipe,
            (Arch::Aarch64, 19) | (Arch::X86_64, 290) => Eventfd2,
            (Arch::X86_64, 284) => Eventfd,
            (Arch::X86_64, 7) => Poll,
            (Arch::Aarch64, 73) | (Arch::X86_64, 271) => Ppoll,
            (Arch::Aarch64, 20) | (Arch::X86_64, 291) => EpollCreate1,
            (Arch::X86_64, 213) => EpollCreate,
            (Arch::Aarch64, 21) | (Arch::X86_64, 233) => EpollCtl,
            (Arch::X86_64, 232) => EpollWait,
            (Arch::Aarch64, 22) | (Arch::X86_64, 281) => EpollPwait,
            (Arch::Aarch64, 198) | (Arch::X86_64, 41) => Socket,
            (Arch::Aarch64, 199) | (Arch::X86_64, 53) => Socketpair,
            (Arch::Aarch64, 200) | (Arch::X86_64, 49) => Bind,
            (Arch::Aarch64, 201) | (Arch::X86_64, 50) => Listen,
            (Arch::Aarch64, 202) | (Arch::X86_64, 43) => Accept,
            (Arch::Aarch64, 242) | (Arch::X86_64, 288) => Accept4,
            (Arch::Aarch64, 203) | (Arch::X86_64, 42) => Connect,
            (Arch::Aarch64, 204) | (Arch::X86_64, 51) => Getsockname,
            (Arch::Aarch64, 205) | (Arch::X86_64, 52) => Getpeername,
            (Arch::Aarch64, 206) | (Arch::X86_64, 44) => Sendto,
            (Arch::Aarch64, 207) | (Arch::X86_64, 45) => Recvfrom,
            (Arch::Aarch64, 208) | (Arch::X86_64, 54) => Setsockopt,
            (Arch::Aarch64, 209) | (Arch::X86_64, 55) => Getsockopt,
            (Arch::Aarch64, 210) | (Arch::X86_64, 48) => Shutdown,
            (Arch::Aarch64, 211) | (Arch::X86_64, 46) => Sendmsg,
            (Arch::Aarch64, 212) | (Arch::X86_64, 47) => Recvmsg,
            (Arch::Aarch64, 220) | (Arch::X86_64, 56) => Clone,
            (Arch::Aarch64, 435) | (Arch::X86_64, 435) => Clone3,
            (Arch::Aarch64, 179) | (Arch::X86_64, 99) => Sysinfo,
            (Arch::Aarch64, 291) | (Arch::X86_64, 332) => Statx,
            (Arch::Aarch64, 123) | (Arch::X86_64, 204) => SchedGetaffinity,
            (Arch::Aarch64, 223) | (Arch::X86_64, 221) => Fadvise64,
            (Arch::Aarch64, 169) | (Arch::X86_64, 96) => Gettimeofday,
            (Arch::Aarch64, 114) | (Arch::X86_64, 229) => ClockGetres,
            (Arch::Aarch64, 90) | (Arch::X86_64, 125) => Capget,
            (Arch::Aarch64, 167) | (Arch::X86_64, 157) => Prctl,
            (Arch::Aarch64, 121) | (Arch::X86_64, 143) => SchedGetparam,
            (Arch::Aarch64, 120) | (Arch::X86_64, 145) => SchedGetscheduler,
            (Arch::Aarch64, 425) | (Arch::X86_64, 425) => IoUringSetup,
            _ => return None,
        })
    }

    /// (number, arguments) from the saved registers.
    fn call(self, cell: &Cell) -> (u64, [u64; 6]) {
        let r = &cell.regs().gpr;
        match self {
            Arch::Aarch64 => (r[8], [r[0], r[1], r[2], r[3], r[4], r[5]]),
            // gpr order: rax rbx rcx rdx rsi rdi rbp r8 r9 r10 ...
            Arch::X86_64 => (r[0], [r[5], r[4], r[3], r[9], r[7], r[8]]),
        }
    }

    /// The number of the system call saved in `cell`'s registers.
    pub fn call_number(self, cell: &Cell) -> u64 {
        self.call(cell).0
    }

    /// Whether call `nr` is a socket call: its time is network time in the
    /// kernel's cost accounting (spec `profiling`).
    pub fn is_socket_call(self, nr: u64) -> bool {
        use Sys::*;
        matches!(
            self.decode(nr),
            Some(
                Socket
                    | Socketpair
                    | Bind
                    | Listen
                    | Accept
                    | Accept4
                    | Connect
                    | Getsockname
                    | Getpeername
                    | Sendto
                    | Recvfrom
                    | Setsockopt
                    | Getsockopt
                    | Shutdown
                    | Sendmsg
                    | Recvmsg
            )
        )
    }

    /// The name of call `nr` (`Read`, `Mmap`…), for reports; `None` for a
    /// call Rhizo does not implement.
    pub fn call_name(self, nr: u64) -> Option<impl core::fmt::Debug> {
        self.decode(nr)
    }

    const fn machine(self) -> &'static [u8] {
        match self {
            Arch::Aarch64 => b"aarch64",
            Arch::X86_64 => b"x86_64",
        }
    }

    /// Length of the system-call instruction (`SVC #0`, `SYSCALL`): a
    /// blocked call runs again from it (design D3 of
    /// `add-sockets-and-poll`).
    const fn syscall_len(self) -> u64 {
        match self {
            Arch::Aarch64 => 4,
            Arch::X86_64 => 2,
        }
    }
}

/// What the kernel provides to system calls besides memory. Its `Console`
/// is the cell's standard output and error.
pub trait Services: Console {
    /// Prints a kernel message about the cell (not part of its output).
    fn log(&mut self, message: fmt::Arguments<'_>);
    /// Nanoseconds since boot.
    fn monotonic_ns(&mut self) -> u64;
    /// Nanoseconds since the Unix epoch.
    fn realtime_ns(&mut self) -> u64;
    /// Fills `out` with random bytes.
    fn random(&mut self, out: &mut [u8]);
    /// Whether the cell may take `bytes` more memory (pages, or kernel-held
    /// data such as file blocks, names and thread records) without eating
    /// into the memory the kernel keeps for itself.
    fn can_store(&mut self, bytes: u64) -> bool;
    /// The memory cells share in bytes, for `sysinfo` (fixed at boot).
    fn memory(&mut self) -> u64 {
        0
    }
    /// Brings `fp`, the calling thread's FP/SIMD state, up to date when the
    /// CPU's registers hold a newer copy (the kernel switches them lazily).
    fn save_fp(&mut self, fp: &mut FpState) {
        let _ = fp;
    }
    /// Cell `cell`'s call was refused `proto` to `ip`:`port` by its egress
    /// policy or the host's deny floor (`EACCES`): the kernel audits it
    /// (design D7 and D18 of `add-cell-hierarchy`).
    fn egress_denied(&mut self, cell: u32, proto: rhizo_net::Proto, ip: [u8; 4], port: u16) {
        let _ = (cell, proto, ip, port);
    }
}

/// What the cell does after a system call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Return to the calling thread (the result is in its registers).
    Resume,
    /// Let other cells run first, then resume the cell.
    Yield,
    /// The calling thread stopped: it waits (a sleep, a futex or
    /// descriptors, recorded in the cell's thread table) or it exited.
    /// Another thread of the cell runs, or the cell waits as its threads
    /// say. A thread waiting on descriptors ([`Wait::Io`]) is back at its
    /// system-call instruction: when it runs again, it makes the call again
    /// (design D3 of `add-sockets-and-poll`).
    Block,
    /// The cell exited with this status (0–255).
    Exit(i32),
}

pub use rhizo_cell::memory::NoPager;

/// Wakes the threads of `cell` waiting on descriptors if its pipes, event
/// counters or `epoll` instances `changed` (kept off the common path).
#[cold]
#[inline(never)]
fn wake_on_fs_change(cell: &mut Cell, changed: bool) {
    if changed {
        cell.threads.wake_io();
    }
}

/// The kernel's sockets as a system call reaches them: the kernel locks
/// them the first time a call uses them, so calls that use no socket never
/// wait for the calls of other cells that do (design D1 of
/// `split-sockets-lock`).
pub trait Sockets {
    /// The sockets, locked until the end of the call.
    fn net(&mut self) -> &mut rhizo_net::Net;
}

impl Sockets for rhizo_net::Net {
    fn net(&mut self) -> &mut rhizo_net::Net {
        self
    }
}

/// Everything a call needs to reach the cell's memory and the kernel.
pub struct Context<'a, F, M, T, A, S> {
    pub pt: PageTable<F>,
    pub mem: &'a mut M,
    pub tlb: &'a mut T,
    pub frames: &'a mut A,
    pub services: &'a mut S,
    /// The cell's files and descriptors.
    pub fs: &'a mut rhizo_fs::Fs,
    /// The sockets of every cell (spec `networking`), reached only by the
    /// calls that use them.
    pub net: &'a mut dyn Sockets,
    /// On entry, the deadline of the blocked call the thread makes again,
    /// if any (it keeps its timeout); a call that blocks on descriptors sets
    /// the deadline it waits for ([`Wait::Io`]).
    pub deadline: Option<u64>,
    /// Brings in pages that are not mapped yet (lazy resume); [`NoPager`]
    /// otherwise.
    pub pager: &'a mut dyn Pager<F, M>,
    /// The frames of mapped file pages.
    pub pages: &'a mut dyn FilePages<M, A>,
}

impl<F, M, T, A, S> fmt::Debug for Context<'_, F, M, T, A, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Context").finish_non_exhaustive()
    }
}

impl<F: EntryFormat, M: PhysicalMemory, T, A, S> Context<'_, F, M, T, A, S> {
    /// Copies cell memory at `addr` into `out`.
    fn read(&mut self, addr: u64, out: &mut [u8]) -> Result<(), memory::Fault> {
        memory::read(&self.pt, self.mem, self.pager, addr, out)
    }

    /// Copies `data` to cell memory at `addr`.
    fn write(&mut self, addr: u64, data: &[u8]) -> Result<(), memory::Fault> {
        memory::write(&self.pt, self.mem, self.pager, addr, data)
    }
}

/// Kernel memory the cell's actions take besides its pages is counted
/// against its memory quota (design D4 of `add-cell-hierarchy`): what its
/// files hold, and its thread records.
impl<F, M, T, A: Frames, S: Services> Context<'_, F, M, T, A, S> {
    /// Counts `bytes` more against the cell's memory, then asks the kernel
    /// whether it has them: `false`, counting nothing, if either refuses.
    /// Counted first, a cell with a memory limit takes them out of what is
    /// set aside for it.
    fn store(&mut self, bytes: u64) -> bool {
        if !self.frames.charge(bytes) {
            return false;
        }
        if self.services.can_store(bytes) {
            return true;
        }
        self.frames.credit(bytes);
        false
    }

    /// Whether the kernel has `bytes` more memory for the cell's pages:
    /// what its memory limit leaves is set aside for it (design D4 of
    /// `add-cell-hierarchy`), so only the rest is asked of the kernel.
    fn can_map(&mut self, bytes: u64) -> bool {
        let room = match self.frames.metered() {
            Some((used, limit)) if limit < Quota::NONE.frames() => {
                limit.saturating_sub(used).saturating_mul(FRAME_SIZE)
            }
            _ => 0,
        };
        bytes <= room || self.services.can_store(bytes - room)
    }

    /// Kernel memory the cell's files hold for it: file blocks and names,
    /// and each socket it holds at its full size (`rhizo_net::SOCKET_BYTES`).
    fn held(&self) -> u64 {
        self.fs.charged() + u64::from(self.fs.socket_count()) * rhizo_net::SOCKET_BYTES
    }

    /// After a change to the cell's files for which [`store`](Self::store)
    /// counted `most` (0 for none) when they held `before`: only what they
    /// hold more, or less, stays counted.
    fn settle(&mut self, most: u64, before: u64) {
        let after = self.held();
        if after == before && most == 0 {
            return; // most calls: nothing to count
        }
        if after < before {
            self.frames.credit(most + (before - after));
        } else if after - before <= most {
            self.frames.credit(most - (after - before));
        } else {
            self.frames.credit(most);
            let _ = self.frames.charge(after - before); // more than asked
        }
    }
}

/// The system-call handler.
#[derive(Debug)]
pub struct Abi {
    arch: Arch,
    /// Unimplemented call numbers already reported: one bit each below 1023
    /// (beyond every Linux call number), and bit 1023 for all larger ones,
    /// so no cell can make the kernel print without end.
    reported: [u64; 16],
    /// Where the cell's ephemeral ports go next, 0 before the first (design
    /// D2 of `close-cross-cell-leaks`): two bytes, so the handler, kept
    /// with every cell, does not grow.
    next_port: u16,
}

/// `AT_FDCWD` as a system-call argument.
const AT_FDCWD: u64 = -100i64 as u64;
/// What a call returns when it must wait ([`Outcome::Block`]); never a
/// Linux result.
const BLOCK: i64 = i64::MIN;
/// Bytes copied per step when moving data between a cell and the kernel.
const CHUNK: usize = 256;
/// Lowest address the stack region (grown to its limit, with a guard page)
/// may reach.
const STACK_FLOOR: u64 = STACK_TOP - STACK_LIMIT - FRAME_SIZE;
const MAP_PRIVATE: u64 = 0x02;
const MAP_FIXED: u64 = 0x10;
const MAP_ANONYMOUS: u64 = 0x20;
const PROT_WRITE: u64 = 2;
const PROT_EXEC: u64 = 4;

impl Abi {
    /// A handler for cells of `arch`.
    pub const fn new(arch: Arch) -> Self {
        Self {
            arch,
            reported: [0; 16],
            next_port: 0,
        }
    }

    /// Runs the system call saved in `cell`'s registers and stores its result
    /// there.
    pub fn handle<F, M, T, A, S>(
        &mut self,
        cell: &mut Cell,
        cx: &mut Context<'_, F, M, T, A, S>,
    ) -> Outcome
    where
        F: EntryFormat,
        M: PhysicalMemory,
        T: Tlb,
        A: Frames,
        S: Services,
    {
        let (nr, a) = self.arch.call(cell);
        // Only another thread of the cell can wait for its pipes, event
        // counters and `epoll` instances (design D3 of
        // `add-socket-wait-queues`).
        let fs_seen = (cell.threads.len() > 1).then(|| cx.fs.changes());
        let result = match self.arch.decode(nr) {
            // One thread of several: only it ends.
            Some(Sys::Exit) if cell.threads.len() > 1 => {
                threads::exit_thread(cell, cx);
                return Outcome::Block;
            }
            Some(Sys::Exit | Sys::ExitGroup) => {
                let status = (a[0] & 0xff) as i32;
                cell.exit_status = Some(status);
                return Outcome::Exit(status);
            }
            Some(Sys::Clone) => {
                let args = threads::clone_args(a, self.arch == Arch::X86_64);
                threads::clone(cell, cx, args)
            }
            Some(Sys::Clone3) => match threads::clone3_args(cx, a[0], a[1]) {
                Ok(args) => threads::clone(cell, cx, args),
                Err(e) => e,
            },
            Some(Sys::Write) => files::write(cx, a[0], a[1], a[2], None),
            Some(Sys::Writev) => writev(cx, a[0], a[1], a[2], false),
            Some(Sys::Readv) => writev(cx, a[0], a[1], a[2], true),
            Some(Sys::Pipe2) => poll::pipe2(cx, a[0], a[1]),
            Some(Sys::Pipe) => poll::pipe2(cx, a[0], 0),
            Some(Sys::Eventfd2) => poll::eventfd2(cx, a[0], a[1]),
            Some(Sys::Eventfd) => poll::eventfd2(cx, a[0], 0),
            Some(Sys::Poll) => {
                let ms = a[2] as i32;
                poll::poll(cx, a[0], a[1], (ms >= 0).then(|| ms as u64 * 1_000_000))
            }
            Some(Sys::Ppoll) => match poll::timespec_timeout(cx, a[2]) {
                Ok(timeout) => poll::poll(cx, a[0], a[1], timeout),
                Err(e) => e,
            },
            Some(Sys::EpollCreate1) => poll::epoll_create(cx, a[0]),
            Some(Sys::EpollCreate) if (a[0] as i32) <= 0 => -EINVAL,
            Some(Sys::EpollCreate) => poll::epoll_create(cx, 0),
            Some(Sys::EpollCtl) => poll::epoll_ctl(cx, self.arch, a[0], a[1], a[2], a[3]),
            Some(Sys::EpollWait | Sys::EpollPwait) => {
                poll::epoll_wait(cx, self.arch, a[0], a[1], a[2], a[3])
            }
            // Sockets belong to the cell's own network (design D5 of
            // `add-cell-hierarchy`).
            Some(Sys::Socket) => {
                self.with_ports(cell, cx, |cx| sockets::socket(cx, a[0], a[1], a[2]))
            }
            Some(Sys::Socketpair) => self.with_ports(cell, cx, |cx| sockets::socketpair(cx, a)),
            Some(Sys::Bind) => self.with_ports(cell, cx, |cx| sockets::bind(cx, a[0], a[1], a[2])),
            Some(Sys::Listen) => self.with_ports(cell, cx, |cx| sockets::listen(cx, a[0], a[1])),
            Some(Sys::Accept) => sockets::accept(cx, a[0], a[1], a[2], 0),
            Some(Sys::Accept4) => sockets::accept(cx, a[0], a[1], a[2], a[3]),
            Some(Sys::Connect) => {
                self.with_ports(cell, cx, |cx| sockets::connect(cx, a[0], a[1], a[2]))
            }
            Some(Sys::Getsockname) => sockets::name(cx, a[0], a[1], a[2], false),
            Some(Sys::Getpeername) => sockets::name(cx, a[0], a[1], a[2], true),
            Some(Sys::Sendto) => self.with_ports(cell, cx, |cx| sockets::sendto(cx, a)),
            Some(Sys::Recvfrom) => sockets::recvfrom(cx, a),
            Some(Sys::Sendmsg) => {
                self.with_ports(cell, cx, |cx| sockets::sendmsg(cx, a[0], a[1], a[2]))
            }
            Some(Sys::Recvmsg) => sockets::recvmsg(cx, a[0], a[1], a[2]),
            Some(Sys::Shutdown) => sockets::shutdown(cx, a[0], a[1]),
            Some(Sys::Setsockopt) => sockets::setsockopt(cx, a),
            Some(Sys::Getsockopt) => sockets::getsockopt(cx, a),
            Some(Sys::Brk) => brk(cell, cx, a[0]),
            Some(Sys::Mmap) => mmap(cell, cx, a),
            Some(Sys::Munmap) => munmap(cx, a[0], a[1]),
            Some(Sys::Getpid) => cell.pid as i64,
            Some(Sys::Gettid) => cell.threads.current().tid as i64,
            Some(Sys::SetTidAddress) => {
                let thread = cell.threads.current_mut();
                thread.clear_tid = a[0];
                thread.tid as i64
            }
            Some(Sys::Uname) => uname(cx, self.arch, a[0]),
            Some(Sys::ClockGettime) => clock_gettime(cx, a[0], a[1]),
            Some(Sys::Time) => time(cx, a[0]),
            Some(Sys::Gettimeofday) => gettimeofday(cx, a[0], a[1]),
            Some(Sys::ClockGetres) => clock_getres(cx, a[0], a[1]),
            Some(Sys::Capget) => capget(cx, a[0], a[1]),
            Some(Sys::Prctl) => prctl(cx, a[0], a[1]),
            Some(Sys::SchedGetparam) => sched_policy(cell, cx, a[0], Some(a[1])),
            Some(Sys::SchedGetscheduler) => sched_policy(cell, cx, a[0], None),
            // Declined on purpose, as `rseq`: libuv falls back to `epoll`.
            Some(Sys::IoUringSetup) => -ENOSYS,
            Some(Sys::Getrandom) => getrandom(cx, a[0], a[1]),
            // Signals are not delivered yet: accept, report no previous state.
            Some(Sys::RtSigaction) => zero_out(cx, a[2], 32),
            Some(Sys::RtSigprocmask) => zero_out(cx, a[2], a[3].min(128)),
            Some(Sys::Ioctl) => files::ioctl(cx, a[0], a[1], a[2]),
            Some(Sys::ArchPrctl) => arch_prctl(cell, cx, a[0], a[1]),
            Some(Sys::Mprotect) => mprotect(cell, cx, a[0], a[1], a[2]),
            Some(Sys::Sysinfo) => sysinfo(cell, cx, a[0]),
            Some(Sys::Statx) => files::statx(cx, a),
            // One CPU: what a cell's threads may count on running at once.
            Some(Sys::SchedGetaffinity) if a[1] < 8 => -EINVAL,
            Some(Sys::SchedGetaffinity) => match put(cx, a[2], &1u64.to_le_bytes()) {
                0 => 8,
                e => e,
            },
            // Advice changes nothing for files in memory.
            Some(Sys::Fadvise64) => match cx.fs.kind(a[0] as u32) {
                Ok(rhizo_fs::Kind::Pipe | rhizo_fs::Kind::Socket) => -ESPIPE,
                Ok(_) => 0,
                Err(e) => -e.errno(),
            },
            // Robust futex lists are never walked (no thread dies holding a
            // lock without its cell ending too: there are no signals).
            Some(Sys::SetRobustList) => 0,
            // Declined on purpose (as on Linux built without it): libc falls
            // back, so it is not reported as missing.
            Some(Sys::Rseq) => -ENOSYS,
            Some(Sys::Prlimit64) => rlimit(cx, &cell.quota, a[1], a[3]),
            Some(Sys::Getrlimit) => rlimit(cx, &cell.quota, a[0], a[1]),
            // No symbolic links exist.
            Some(Sys::Readlink | Sys::Readlinkat) => -ENOENT,
            // Cells run as root of their own world: uid, gid 0; no parent.
            Some(Sys::Ids | Sys::Getppid) => 0,
            Some(Sys::Sigaltstack) => 0,
            Some(Sys::Madvise) => madvise(cx, a[0], a[1], a[2]),
            Some(Sys::Futex) => match threads::futex(cell, cx, a) {
                Some(result) => result,
                None => return Outcome::Block,
            },
            Some(Sys::Fstat) => files::fstat(cx, self.arch, a[0], a[1]),
            Some(Sys::Newfstatat) => files::newfstatat(cx, self.arch, a[0], a[1], a[2], a[3]),
            Some(Sys::Stat | Sys::Lstat) => {
                files::newfstatat(cx, self.arch, AT_FDCWD, a[0], a[1], 0)
            }
            Some(Sys::Openat) => files::openat(cx, self.arch, cell, a[0], a[1], a[2]),
            Some(Sys::Open) => files::openat(cx, self.arch, cell, AT_FDCWD, a[0], a[1]),
            Some(Sys::Close) => files::close(cx, a[0]),
            Some(Sys::Read) => files::read(cx, a[0], a[1], a[2], None),
            Some(Sys::Pread) => match a[3] as i64 {
                at if at < 0 => -EINVAL,
                at => files::read(cx, a[0], a[1], a[2], Some(at as u64)),
            },
            Some(Sys::Pwrite) => match a[3] as i64 {
                at if at < 0 => -EINVAL,
                at => files::write(cx, a[0], a[1], a[2], Some(at as u64)),
            },
            Some(Sys::Lseek) => files::lseek(cx, a[0], a[1], a[2]),
            Some(Sys::Getdents64) => files::getdents64(cx, a[0], a[1], a[2]),
            Some(Sys::Mkdirat) => files::mkdirat(cx, a[0], a[1]),
            Some(Sys::Mkdir) => files::mkdirat(cx, AT_FDCWD, a[0]),
            Some(Sys::Unlinkat) => files::unlinkat(cx, a[0], a[1], a[2]),
            Some(Sys::Unlink) => files::unlinkat(cx, AT_FDCWD, a[0], 0),
            Some(Sys::Rmdir) => files::unlinkat(cx, AT_FDCWD, a[0], 0x200),
            Some(Sys::Renameat) => files::renameat(cx, a, 0),
            Some(Sys::Renameat2) => files::renameat(cx, a, a[4]),
            Some(Sys::Rename) => files::renameat(cx, [AT_FDCWD, a[0], AT_FDCWD, a[1], 0, 0], 0),
            Some(Sys::Getcwd) => files::getcwd(cx, a[0], a[1]),
            Some(Sys::Chdir) => files::chdir(cx, a[0]),
            Some(Sys::Ftruncate) => files::ftruncate(cx, a[0], a[1]),
            Some(Sys::Fsync) => files::fsync(cx, a[0]),
            Some(Sys::Dup) => files::dup(cx, a[0], 0, false, false),
            Some(Sys::Dup2) => match cx.fs.kind(a[0] as u32) {
                Ok(_) if a[0] as u32 == a[1] as u32 => (a[1] as u32).into(),
                _ => files::dup(cx, a[0], a[1], true, false),
            },
            Some(Sys::Dup3) => files::dup3(cx, a[0], a[1], a[2]),
            Some(Sys::Fcntl) => files::fcntl(cx, a[0], a[1], a[2]),
            Some(Sys::Faccessat) => files::faccessat(cx, a[0], a[1], a[2]),
            Some(Sys::Faccessat2) => files::faccessat(cx, a[0], a[1], a[2]),
            Some(Sys::Access) => files::faccessat(cx, AT_FDCWD, a[0], a[1]),
            Some(Sys::SchedYield) => {
                cell.regs_mut().gpr[0] = 0;
                return Outcome::Yield;
            }
            Some(Sys::Nanosleep) => match sleep_until(cx, 1, 0, a[0]) {
                Ok(until) => return sleep(cell, until),
                Err(e) => e,
            },
            Some(sys @ (Sys::Select | Sys::Pselect6)) if a[0] != 0 => {
                let timeout = if sys == Sys::Select {
                    poll::timeval_timeout(cx, a[4])
                } else {
                    poll::timespec_timeout(cx, a[4])
                };
                match timeout {
                    Ok(timeout) => poll::select(cx, a, timeout),
                    Err(e) => e,
                }
            }
            // Without descriptors, `select` is a sleep (how MicroPython's
            // `time.sleep` waits).
            Some(sys @ (Sys::Select | Sys::Pselect6)) => {
                match select_sleep(cx, a[4], sys == Sys::Select) {
                    Ok(until) => return sleep(cell, until),
                    Err(e) => e,
                }
            }
            Some(Sys::ClockNanosleep) => match sleep_until(cx, a[0], a[1], a[2]) {
                Ok(until) => return sleep(cell, until),
                Err(e) => e,
            },
            None => {
                self.report_unimplemented(cell.id, nr, cx.services);
                -ENOSYS
            }
        };
        // Sockets whose last descriptor the call closed.
        if cx.fs.has_released() {
            for h in cx.fs.take_released() {
                cx.net.net().release(h);
            }
        }
        // Another thread of the cell may wait for what changed in its pipes,
        // event counters or `epoll` instances (design D3 of
        // `add-socket-wait-queues`).
        if let Some(seen) = fs_seen {
            wake_on_fs_change(cell, cx.fs.changes() != seen);
        }
        if result == BLOCK {
            cell.regs_mut().pc -= self.arch.syscall_len();
            cell.threads.block(Wait::Io { until: cx.deadline });
            return Outcome::Block;
        }
        // The result goes in x0 / rax, which is gpr[0] on both.
        cell.regs_mut().gpr[0] = result as u64;
        Outcome::Resume
    }

    /// Runs `call`, a socket call that may make a socket or bind an
    /// ephemeral port, with the cell's own network and ports entered into
    /// the sockets (design D2 of `close-cross-cell-leaks`, D5 of
    /// `add-cell-hierarchy`).
    fn with_ports<'c, F, M, T, A: Frames, S: Services>(
        &mut self,
        cell: &mut Cell,
        cx: &mut Context<'c, F, M, T, A, S>,
        call: impl FnOnce(&mut Context<'c, F, M, T, A, S>) -> i64,
    ) -> i64 {
        // A cell with a memory limit is in a reserved domain, whose sockets
        // were admitted (design D4 of `add-cell-hierarchy`).
        let reserved = cx
            .frames
            .metered()
            .is_some_and(|(_, limit)| limit < Quota::NONE.frames());
        let ports = Ports {
            next: self.next_port,
            reserved,
            ..Ports::new(cell.id)
        };
        cx.net.net().enter(ports);
        let result = call(cx);
        let ports = cx.net.net().leave();
        self.next_port = ports.map_or(0, |p| p.next);
        // A cell that reached the network card may hold a secret from
        // outside (design D22 of `add-cell-hierarchy`).
        cell.exposed |= ports.is_some_and(|p| p.out);
        if let Some((proto, ip, port)) = ports.and_then(|p| p.denied) {
            cx.services.egress_denied(cell.id, proto, ip, port);
        }
        result
    }

    fn report_unimplemented(&mut self, cell: u32, nr: u64, out: &mut impl Services) {
        let bit = nr.min(1023);
        let word = &mut self.reported[(bit / 64) as usize];
        if *word & (1 << (bit % 64)) != 0 {
            return;
        }
        *word |= 1 << (bit % 64);
        out.log(format_args!("cell {cell}: unimplemented syscall {nr}"));
    }
}

/// The calling thread sleeps until monotonic time `until`; the call
/// returns 0.
fn sleep(cell: &mut Cell, until: u64) -> Outcome {
    cell.regs_mut().gpr[0] = 0;
    cell.threads.block(Wait::Sleep(until));
    Outcome::Block
}

/// `writev`, or `readv` with `read`.
fn writev<F: EntryFormat, M: PhysicalMemory, T, A: Frames, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    iov: u64,
    count: u64,
    read: bool,
) -> i64 {
    if count > 1024 {
        return -EINVAL;
    }
    let mut total = 0;
    for i in 0..count {
        let mut entry = [0u8; 16];
        if cx.read(iov.wrapping_add(i * 16), &mut entry).is_err() {
            return if total == 0 { -EFAULT } else { total };
        }
        let base = u64::from_le_bytes(entry[..8].try_into().unwrap_or_default());
        let len = u64::from_le_bytes(entry[8..].try_into().unwrap_or_default());
        let n = if read {
            files::read(cx, fd, base, len, None)
        } else {
            files::write(cx, fd, base, len, None)
        };
        if n < 0 {
            return if total == 0 { n } else { total };
        }
        total += n;
        if (n as u64) < len {
            break;
        }
    }
    total
}

fn user_rw() -> PageFlags {
    PageFlags::USER | PageFlags::WRITABLE | PageFlags::NO_EXECUTE
}

fn brk<F: EntryFormat, M: PhysicalMemory, T: Tlb, A: Frames, S: Services>(
    cell: &mut Cell,
    cx: &mut Context<'_, F, M, T, A, S>,
    want: u64,
) -> i64 {
    if want < cell.brk_start || want > MMAP_BASE {
        return cell.brk as i64;
    }
    let (old_end, new_end) = (align_up(cell.brk, FRAME_SIZE), align_up(want, FRAME_SIZE));
    // Memory that cannot be had is refused before any of it is zeroed.
    let ok = if new_end > old_end {
        cx.can_map(new_end - old_end)
            && space::map_anonymous(
                &cx.pt,
                cx.mem,
                cx.tlb,
                cx.frames,
                old_end,
                new_end - old_end,
                user_rw(),
            )
            .is_ok()
    } else {
        space::unmap(
            &cx.pt,
            cx.mem,
            cx.tlb,
            cx.frames,
            new_end,
            old_end - new_end,
        )
        .is_ok()
    };
    if ok {
        cell.brk = want;
    }
    cell.brk as i64
}

/// Whether `prot` asks for writable and executable memory.
fn is_wx(prot: u64) -> bool {
    prot & (PROT_WRITE | PROT_EXEC) == PROT_WRITE | PROT_EXEC
}

/// The page permissions of `prot` (`PROT_READ` is implied): writable and
/// executable make a write-or-execute page, writing (change `node-jit`).
fn page_flags(prot: u64) -> PageFlags {
    if is_wx(prot) {
        return space::WX_WRITING;
    }
    let mut pf = PageFlags::USER;
    if prot & PROT_WRITE != 0 {
        pf = pf | PageFlags::WRITABLE;
    }
    if prot & PROT_EXEC == 0 {
        pf = pf | PageFlags::NO_EXECUTE;
    }
    pf
}

/// The write-or-execute ranges `wx` once `addr..end` is taken out of them,
/// and added back when `add`, or `None` when nothing changes; checked
/// before anything else changes (design D5 of `node-jit`). When `unmap`,
/// the range also leaves the wipe ranges (design D22 of
/// `add-cell-hierarchy`). A cell whose pager keeps no ranges (`None`) can
/// have none.
fn wx_after(
    wx: Option<&mut WxRanges>,
    addr: u64,
    end: u64,
    add: bool,
    unmap: bool,
) -> Result<Option<WxRanges>, TooManyRanges> {
    let Some(wx) = wx else {
        return if add { Err(TooManyRanges) } else { Ok(None) };
    };
    if !add && wx.ranges().is_empty() && (!unmap || wx.wiped().is_empty()) {
        return Ok(None);
    }
    let mut after = wx.clone();
    after.remove(addr, end)?;
    if unmap {
        after.keep(addr, end)?;
    }
    if add {
        after.add(addr, end)?;
    }
    Ok(Some(after))
}

/// Makes `ranges` (from [`wx_after`]) the cell's write-or-execute ranges.
fn set_wx<F: EntryFormat, M: PhysicalMemory>(
    pager: &mut dyn Pager<F, M>,
    ranges: Option<WxRanges>,
) {
    if let (Some(ranges), Some(wx)) = (ranges, pager.wx()) {
        *wx = ranges;
    }
}

/// `mmap`: anonymous private memory, or a private mapping of a file's
/// pages (spec `linux-abi`, file mappings), at the next free address or,
/// with `MAP_FIXED`, in place of whatever was mapped at the address given.
/// Anonymous memory both writable and executable becomes a write-or-execute
/// range, backed on first touch (design D5 of `node-jit`).
fn mmap<F: EntryFormat, M: PhysicalMemory, T: Tlb, A: Frames, S: Services>(
    cell: &mut Cell,
    cx: &mut Context<'_, F, M, T, A, S>,
    a: [u64; 6],
) -> i64 {
    const MAP_SHARED: u64 = 0x01;
    let (hint, len, prot, flags, fd, offset) = (a[0], a[1], a[2], a[3], a[4], a[5]);
    let anonymous = flags & MAP_ANONYMOUS != 0;
    if flags & MAP_SHARED != 0 && flags & MAP_PRIVATE == 0 && !anonymous {
        return -ENODEV; // shared file mappings are not supported
    }
    if len == 0 || flags & (MAP_SHARED | MAP_PRIVATE) != MAP_PRIVATE {
        return -EINVAL;
    }
    if is_wx(prot) && !anonymous {
        return -EACCES; // writable and executable: anonymous memory only
    }
    let Some(len) = len
        .checked_add(FRAME_SIZE - 1)
        .map(|l| l & !(FRAME_SIZE - 1))
    else {
        return -ENOMEM;
    };
    // The file and its first page, checked before anything changes.
    let file = if anonymous {
        None
    } else {
        if !offset.is_multiple_of(FRAME_SIZE) {
            return -EINVAL;
        }
        match (cx.fs.kind(fd as u32), cx.fs.status_flags(fd as u32)) {
            (Err(_), _) => return -EBADF,
            (Ok(rhizo_fs::Kind::File), Ok(f)) if f & 3 != 1 => {}
            (Ok(rhizo_fs::Kind::File), _) => return -EACCES, // write-only
            _ => return -ENODEV,
        }
        match cx.fs.file(fd as u32) {
            Ok((ino, _)) => Some((ino, offset / FRAME_SIZE)),
            Err(e) => return -e.errno(),
        }
    };
    let fixed = flags & MAP_FIXED != 0;
    let addr = if fixed {
        if !hint.is_multiple_of(FRAME_SIZE) || hint.checked_add(len).is_none_or(|e| e > USER_END) {
            return -EINVAL;
        }
        if hint < FRAME_SIZE {
            return -EPERM;
        }
        hint
    } else {
        let align = if len >= LARGE_PAGE_SIZE && anonymous {
            LARGE_PAGE_SIZE
        } else {
            FRAME_SIZE
        };
        let addr = align_up(cell.mmap_next, align);
        if addr.checked_add(len).is_none_or(|end| end > STACK_FLOOR) {
            return -ENOMEM;
        }
        addr
    };
    let wx = is_wx(prot);
    if prot != 0 && !wx && anonymous && !cx.can_map(len) {
        return -ENOMEM; // refused before any of it is zeroed
    }
    let Ok(ranges) = wx_after(cx.pager.wx(), addr, addr + len, wx, fixed) else {
        return -ENOMEM;
    };
    if fixed {
        // What was mapped there goes, as on Linux.
        if space::unmap(&cx.pt, cx.mem, cx.tlb, cx.frames, addr, len).is_err() {
            return -ENOMEM;
        }
        cx.pager.discard(addr, len);
    }
    set_wx(cx.pager, ranges);
    let pf = page_flags(prot);
    let mapped = match file {
        // Reserved (spec `linux-abi`), or a write-or-execute range.
        _ if prot == 0 || wx => Ok(()),
        None => space::map_anonymous(&cx.pt, cx.mem, cx.tlb, cx.frames, addr, len, pf),
        Some((ino, first)) => {
            let (fs, pages) = (&*cx.fs, &mut *cx.pages);
            space::map_frames(
                &cx.pt,
                cx.mem,
                cx.tlb,
                cx.frames,
                addr,
                len,
                pf,
                &mut |i, mem, frames| {
                    let page = fs.page(ino, first + i).ok()?;
                    pages.frame(&page, cell.domain, mem, frames)
                },
            )
        }
    };
    if mapped.is_err() {
        return -ENOMEM;
    }
    if !fixed || (addr >= MMAP_BASE && addr + len > cell.mmap_next && addr + len <= STACK_FLOOR) {
        cell.mmap_next = addr + len;
    }
    addr as i64
}

fn munmap<F: EntryFormat, M: PhysicalMemory, T: Tlb, A: Frames, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
    len: u64,
) -> i64 {
    let end = align_up(addr.saturating_add(len), FRAME_SIZE);
    if len == 0 || !addr.is_multiple_of(FRAME_SIZE) || end > STACK_TOP {
        return -EINVAL;
    }
    let Ok(ranges) = wx_after(cx.pager.wx(), addr, end, false, true) else {
        return -ENOMEM; // as Linux, when a split needs one area too many
    };
    match space::unmap(&cx.pt, cx.mem, cx.tlb, cx.frames, addr, end - addr) {
        Ok(()) => {
            cx.pager.discard(addr, end - addr);
            set_wx(cx.pager, ranges);
            0
        }
        Err(_) => -EINVAL,
    }
}

/// `madvise`: advice is taken and ignored, except `MADV_WIPEONFORK` and
/// `MADV_KEEPONFORK` (design D22 of `add-cell-hierarchy`), which add the
/// cell's memory in `addr..addr + len` to its wipe ranges, which read as
/// zero in every copy of the cell, or take it out. As on Linux, the range
/// must be mapped (`ENOMEM`); here no 2 MiB page may lie partly outside it
/// (`EINVAL`).
fn madvise<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
    len: u64,
    advice: u64,
) -> i64 {
    const MADV_WIPEONFORK: u64 = 18;
    const MADV_KEEPONFORK: u64 = 19;
    if advice != MADV_WIPEONFORK && advice != MADV_KEEPONFORK {
        return 0;
    }
    let Some(end) = addr
        .checked_add(len)
        .map(|e| align_up(e, FRAME_SIZE))
        .filter(|&e| e <= USER_END && addr.is_multiple_of(FRAME_SIZE))
    else {
        return -EINVAL;
    };
    let mut at = addr;
    while at < end {
        // Pages still in a snapshot, or of a stack still to grow, first.
        let t = cx.pt.translate(cx.mem, at).or_else(|| {
            cx.pager
                .page_in(&cx.pt, cx.mem, at, false)
                .then(|| cx.pt.translate(cx.mem, at))?
        });
        let Some(t) = t else {
            return -ENOMEM;
        };
        let base = at & !(t.size.bytes() - 1);
        if base < addr || base + t.size.bytes() > end {
            return -EINVAL;
        }
        at = base + t.size.bytes();
    }
    let Some(marks) = cx.pager.wx() else {
        return -EINVAL;
    };
    let done = if advice == MADV_WIPEONFORK {
        marks.wipe(addr, end)
    } else {
        marks.keep(addr, end)
    };
    if done.is_ok() { 0 } else { -ENOMEM }
}

/// `sysinfo`, as the cell sees itself (design D1 of
/// `close-cross-cell-leaks`): uptime; its memory limit as the total and
/// what it leaves as free; its threads as the processes; no swap, no load.
/// Nothing in it depends on other cells.
fn sysinfo<F: EntryFormat, M: PhysicalMemory, T, A: Frames, S: Services>(
    cell: &Cell,
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
) -> i64 {
    let (used, limit) = cx.frames.metered().unwrap_or((0, u64::MAX));
    let total = cx.services.memory().min(limit.saturating_mul(FRAME_SIZE));
    let free = total.saturating_sub(used * FRAME_SIZE);
    let procs = cell.threads.len().min(u16::MAX as usize) as u16;
    let mut info = [0u8; 112];
    let uptime = cx.services.monotonic_ns() / 1_000_000_000;
    info[..8].copy_from_slice(&uptime.to_le_bytes());
    info[32..40].copy_from_slice(&total.to_le_bytes());
    info[40..48].copy_from_slice(&free.to_le_bytes());
    info[80..82].copy_from_slice(&procs.to_le_bytes());
    info[104..108].copy_from_slice(&1u32.to_le_bytes()); // mem_unit
    put(cx, addr, &info)
}

fn put<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
    data: &[u8],
) -> i64 {
    match cx.write(addr, data) {
        Ok(()) => 0,
        Err(_) => -EFAULT,
    }
}

fn zero_out<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
    len: u64,
) -> i64 {
    if addr == 0 {
        return 0;
    }
    put(cx, addr, &[0; 128][..len.min(128) as usize])
}

fn uname<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    arch: Arch,
    addr: u64,
) -> i64 {
    // struct utsname: six 65-byte NUL-terminated fields.
    let fields: [&[u8]; 6] = [
        b"Linux",
        b"rhizo",
        b"6.1.0-rhizo",
        b"rhizo",
        arch.machine(),
        b"(none)",
    ];
    let mut buf = [0u8; 6 * 65];
    for (i, f) in fields.iter().enumerate() {
        buf[i * 65..i * 65 + f.len()].copy_from_slice(f);
    }
    put(cx, addr, &buf)
}

fn clock_gettime<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    clock: u64,
    addr: u64,
) -> i64 {
    let ns = match clock {
        0 | 5 => cx.services.realtime_ns(), // REALTIME, REALTIME_COARSE
        1 | 4 | 6 | 7 => cx.services.monotonic_ns(), // MONOTONIC, _RAW, _COARSE, BOOTTIME
        _ => return -EINVAL,
    };
    let mut ts = [0u8; 16];
    ts[..8].copy_from_slice(&(ns / 1_000_000_000).to_le_bytes());
    ts[8..].copy_from_slice(&(ns % 1_000_000_000).to_le_bytes());
    put(cx, addr, &ts)
}

/// `time` (x86_64): whole seconds since the epoch, also stored at `addr`
/// unless it is null.
fn time<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
) -> i64 {
    let now = (cx.services.realtime_ns() / 1_000_000_000) as i64;
    if addr != 0 && cx.write(addr, &now.to_le_bytes()).is_err() {
        return -EFAULT;
    }
    now
}

/// `gettimeofday`: the time since the epoch as a `timeval` at `tv`, and an
/// empty `timezone` at `tz` (each unless null).
fn gettimeofday<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    tv: u64,
    tz: u64,
) -> i64 {
    let ns = cx.services.realtime_ns();
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&(ns / 1_000_000_000).to_le_bytes());
    out[8..].copy_from_slice(&(ns % 1_000_000_000 / 1000).to_le_bytes());
    match (tv, tz) {
        (0, _) => {}
        _ if put(cx, tv, &out) != 0 => return -EFAULT,
        _ => {}
    }
    if tz != 0 { put(cx, tz, &[0; 8]) } else { 0 }
}

/// `clock_getres`: every clock `clock_gettime` reads counts nanoseconds.
fn clock_getres<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    clock: u64,
    addr: u64,
) -> i64 {
    match clock {
        0 | 1 | 4..=7 if addr == 0 => 0,
        0 | 1 | 4..=7 => put(cx, addr, &[0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]),
        _ => -EINVAL,
    }
}

/// `capget`: a cell holds no capabilities (nothing outside it to be
/// privileged over). Versions 1 to 3 of the header are understood; another
/// is answered with version 3, as Linux does.
fn capget<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    header: u64,
    data: u64,
) -> i64 {
    const V1: u32 = 0x1998_0330;
    const V3: u32 = 0x2008_0522;
    let mut head = [0u8; 8];
    if cx.read(header, &mut head).is_err() {
        return -EFAULT;
    }
    let sets = match u32::from_le_bytes([head[0], head[1], head[2], head[3]]) {
        V1 => 1,
        0x2007_1026 | V3 => 2,
        _ => {
            return match put(cx, header, &V3.to_le_bytes()) {
                0 if data == 0 => 0,
                0 => -EINVAL,
                e => e,
            };
        }
    };
    // Each set: effective, permitted, inheritable (32 bits each).
    if data == 0 {
        0
    } else {
        put(cx, data, &[0; 24][..12 * sets])
    }
}

/// `prctl`: a thread may name itself (the name is not kept: nothing reads
/// it back but `PR_GET_NAME`, which finds it empty); any other option is
/// unknown (`EINVAL`).
fn prctl<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    option: u64,
    arg: u64,
) -> i64 {
    const PR_SET_NAME: u64 = 15;
    const PR_GET_NAME: u64 = 16;
    match option {
        PR_SET_NAME => 0,
        PR_GET_NAME => put(cx, arg, &[0; 16]),
        _ => -EINVAL,
    }
}

/// `sched_getparam` (with `param`) and `sched_getscheduler` of thread
/// `pid` (0: the caller): every thread has the normal policy
/// (`SCHED_OTHER`, 0) and priority 0.
fn sched_policy<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cell: &Cell,
    cx: &mut Context<'_, F, M, T, A, S>,
    pid: u64,
    param: Option<u64>,
) -> i64 {
    let known =
        |id: u64| id == u64::from(cell.pid) || cell.threads.iter().any(|t| u64::from(t.tid) == id);
    match param {
        _ if (pid as i64) < 0 => -EINVAL,
        _ if pid != 0 && !known(pid) => -ESRCH,
        Some(addr) => put(cx, addr, &[0; 4]),
        None => 0,
    }
}

fn getrandom<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
    len: u64,
) -> i64 {
    let mut done = 0;
    while done < len {
        let n = (len - done).min(CHUNK as u64) as usize;
        let mut buf = [0u8; CHUNK];
        cx.services.random(&mut buf[..n]);
        if cx.write(addr.wrapping_add(done), &buf[..n]).is_err() {
            return if done == 0 { -EFAULT } else { done as i64 };
        }
        done += n as u64;
    }
    done as i64
}

/// Monotonic deadline of a sleep on `clock` for the `timespec` at `addr`
/// (relative, or absolute with `TIMER_ABSTIME` in `flags`).
fn sleep_until<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    clock: u64,
    flags: u64,
    addr: u64,
) -> Result<u64, i64> {
    const TIMER_ABSTIME: u64 = 1;
    let value = timespec_ns(cx, addr)?;
    let now = cx.services.monotonic_ns();
    let realtime = match clock {
        0 => true,      // CLOCK_REALTIME
        1 | 7 => false, // CLOCK_MONOTONIC, CLOCK_BOOTTIME
        _ => return Err(-EINVAL),
    };
    Ok(if flags & TIMER_ABSTIME == 0 {
        now.saturating_add(value)
    } else if realtime {
        now.saturating_add(value.saturating_sub(cx.services.realtime_ns()))
    } else {
        value
    })
}

/// The `timespec` at `addr` in nanoseconds: `-EFAULT` if unmapped,
/// `-EINVAL` if negative or with nanoseconds out of range.
fn timespec_ns<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
) -> Result<u64, i64> {
    let mut ts = [0u8; 16];
    cx.read(addr, &mut ts).map_err(|_| -EFAULT)?;
    let sec = i64::from_le_bytes(ts[..8].try_into().unwrap_or_default());
    let nsec = i64::from_le_bytes(ts[8..].try_into().unwrap_or_default());
    if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
        return Err(-EINVAL);
    }
    Ok((sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(nsec as u64))
}

/// Deadline of a `select`/`pselect6` with no descriptors: its timeout is a
/// `timeval` (microseconds, `select`) or a `timespec` (`pselect6`). A null
/// timeout would wait forever for nothing, which no caller means.
fn select_sleep<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
    micros: bool,
) -> Result<u64, i64> {
    if addr == 0 {
        return Err(-EINVAL);
    }
    let mut t = [0u8; 16];
    cx.read(addr, &mut t).map_err(|_| -EFAULT)?;
    let sec = i64::from_le_bytes(t[..8].try_into().unwrap_or_default());
    let frac = i64::from_le_bytes(t[8..].try_into().unwrap_or_default());
    let limit = if micros { 1_000_000 } else { 1_000_000_000 };
    if sec < 0 || !(0..limit).contains(&frac) {
        return Err(-EINVAL);
    }
    let ns = (sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(frac as u64 * (1_000_000_000 / limit as u64));
    Ok(cx.services.monotonic_ns().saturating_add(ns))
}

fn mprotect<F: EntryFormat, M: PhysicalMemory, T: Tlb, A: Frames, S>(
    cell: &Cell,
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
    len: u64,
    prot: u64,
) -> i64 {
    const PROT_READ: u64 = 1;
    if !addr.is_multiple_of(FRAME_SIZE) || prot & !(PROT_READ | PROT_WRITE | PROT_EXEC) != 0 {
        return -EINVAL;
    }
    // Only the cell's own addresses (spec `isolation`): on x86_64 its table
    // also holds the kernel half, whose pages must never become the cell's.
    let Some(end) = addr
        .checked_add(len)
        .map(|e| align_up(e, FRAME_SIZE))
        .filter(|&e| e <= USER_END)
    else {
        return -ENOMEM;
    };
    // Write-or-execute ranges lose what this call covers, or take it all
    // (design D5 of `node-jit`); checked before anything changes.
    let Ok(ranges) = wx_after(cx.pager.wx(), addr, end, is_wx(prot), false) else {
        return -ENOMEM;
    };
    if is_wx(prot) {
        return match protect_wx(cell, cx, addr, end) {
            Ok(()) => {
                set_wx(cx.pager, Some(ranges.unwrap_or_default()));
                0
            }
            Err(e) => e,
        };
    }
    if prot == 0 {
        // Inaccessible pages hold nothing: their memory goes, and in the
        // mapping area their addresses stay reserved (design D4 of
        // `add-dynamic-linking`).
        return match space::unmap(&cx.pt, cx.mem, cx.tlb, cx.frames, addr, end - addr) {
            Ok(()) => {
                cx.pager.discard(addr, end - addr);
                set_wx(cx.pager, ranges);
                0
            }
            Err(_) => -ENOMEM,
        };
    }
    set_wx(cx.pager, ranges);
    let flags = page_flags(prot);
    // Pages not in memory yet (lazy resume) are brought in first; holes
    // below the mapping cursor are reserved (`PROT_NONE`) addresses, backed
    // now (spec `linux-abi`, reserved mappings; design D6 of `add-threads`).
    // Any other missing page is the answer (`ENOMEM`). Each step is a page
    // the cell has, or one more frame it gets (in batches, until memory runs
    // out): a huge range of nothing costs one step (spec `isolation`).
    // Batches end on a 2 MiB boundary, so large pages are used where they fit.
    const BATCH: u64 = 64 * LARGE_PAGE_SIZE;
    let reserved = MMAP_BASE..cell.mmap_next;
    let present = |cx: &mut Context<'_, F, M, T, A, S>, at: u64| {
        cx.pt.translate(cx.mem, at).is_some() || cx.pager.page_in(&cx.pt, cx.mem, at, false)
    };
    let mut at = addr;
    while at < end {
        if present(cx, at) {
            at += FRAME_SIZE;
            continue;
        }
        if !reserved.contains(&at) {
            return -ENOMEM;
        }
        let hole = at;
        let batch_end =
            (hole.saturating_add(BATCH) & !(LARGE_PAGE_SIZE - 1)).max(hole + FRAME_SIZE);
        let limit = end.min(cell.mmap_next).min(batch_end);
        while at < limit && !present(cx, at) {
            at += FRAME_SIZE;
        }
        if space::map_anonymous(&cx.pt, cx.mem, cx.tlb, cx.frames, hole, at - hole, flags).is_err()
        {
            return -ENOMEM;
        }
    }
    match space::protect(&cx.pt, cx.mem, cx.tlb, cx.frames, addr, end - addr, flags) {
        Ok(()) => 0,
        Err(space::ProtectError::PartialLargePage(_)) => -EINVAL,
        Err(_) => -ENOMEM,
    }
}

/// `mprotect` with `PROT_WRITE|PROT_EXEC` over `addr..end` (design D5 of
/// `node-jit`): every page the cell has (brought in first if still pending)
/// becomes a write-or-execute page, writing, keeping its frame; the holes
/// must be reserved addresses, which take no memory (the caller adds them
/// to the write-or-execute ranges). Every page is checked before one
/// changes; a hole costs one step per empty table entry unless pages are
/// still pending (spec `isolation`).
fn protect_wx<F: EntryFormat, M: PhysicalMemory, T: Tlb, A: Frames, S>(
    cell: &Cell,
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
    end: u64,
) -> Result<(), i64> {
    let reserved = MMAP_BASE..cell.mmap_next;
    let pending = cx.pager.has_pending();
    let mut at = addr;
    while at < end {
        if cx.pt.translate(cx.mem, at).is_none() && !cx.pager.page_in(&cx.pt, cx.mem, at, false) {
            let hole_end = match pending {
                true => at + FRAME_SIZE,
                false => cx.pt.hole_end(cx.mem, at).unwrap_or(at + FRAME_SIZE),
            };
            if at < reserved.start || hole_end.min(end) > reserved.end {
                return Err(-ENOMEM);
            }
            at = hole_end;
            continue;
        }
        let size = cx
            .pt
            .translate(cx.mem, at)
            .map_or(FRAME_SIZE, |t| t.size.bytes());
        let base = at & !(size - 1);
        if base < addr || base + size > end {
            return Err(-EINVAL); // part of a 2 MiB page
        }
        at = base + size;
    }
    let mut at = addr;
    while at < end {
        let Some(t) = cx.pt.translate(cx.mem, at) else {
            at = cx.pt.hole_end(cx.mem, at).unwrap_or(at + FRAME_SIZE);
            continue;
        };
        let size = t.size.bytes();
        space::protect(
            &cx.pt,
            cx.mem,
            cx.tlb,
            cx.frames,
            at,
            size,
            space::WX_WRITING,
        )
        .map_err(|_| -ENOMEM)?;
        at += size;
    }
    Ok(())
}

/// Makes the current call wait (design D3 of `add-sockets-and-poll`) for at
/// most `timeout` nanoseconds (`None`: no limit), keeping the deadline of a
/// call made again. Returns [`BLOCK`], or `None` once the deadline passed.
fn block<F, M, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    timeout: Option<u64>,
) -> Option<i64> {
    let now = cx.services.monotonic_ns();
    let deadline = cx
        .deadline
        .or_else(|| timeout.map(|t| now.saturating_add(t)));
    if deadline.is_some_and(|d| d <= now) {
        return None;
    }
    cx.deadline = deadline;
    Some(BLOCK)
}

/// `getrlimit`/`prlimit64` (no new limit is taken): an 8 MiB stack, and the
/// cell's quotas (spec `isolation`) as CPU seconds, address space and
/// files; everything else unlimited.
fn rlimit<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    quota: &Quota,
    resource: u64,
    old: u64,
) -> i64 {
    const INFINITY: u64 = u64::MAX;
    let cpu_s = match quota.cpu_ns {
        INFINITY => INFINITY,
        ns => ns.div_ceil(1_000_000_000),
    };
    let (cur, max) = match resource {
        0 => (cpu_s, cpu_s),                               // RLIMIT_CPU
        3 => (STACK_LIMIT, INFINITY),                      // RLIMIT_STACK
        7 => (quota.files() as u64, quota.files() as u64), // RLIMIT_NOFILE
        9 => (quota.memory, quota.memory),                 // RLIMIT_AS
        _ if resource <= 15 => (INFINITY, INFINITY),
        _ => return -EINVAL,
    };
    if old == 0 {
        return 0;
    }
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&cur.to_le_bytes());
    out[8..].copy_from_slice(&max.to_le_bytes());
    put(cx, old, &out)
}

fn arch_prctl<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cell: &mut Cell,
    cx: &mut Context<'_, F, M, T, A, S>,
    code: u64,
    addr: u64,
) -> i64 {
    const ARCH_SET_FS: u64 = 0x1002;
    const ARCH_GET_FS: u64 = 0x1003;
    match code {
        // A kernel or non-canonical base would fault when loaded into the
        // FS base, in the kernel (as on Linux: `EPERM`).
        ARCH_SET_FS if addr >= USER_END => -EPERM,
        ARCH_SET_FS => {
            cell.regs_mut().tls = addr;
            0
        }
        ARCH_GET_FS => put(cx, addr, &cell.regs().tls.to_le_bytes()),
        _ => -EINVAL,
    }
}

#[cfg(test)]
mod hostile;
#[cfg(test)]
mod net_tests;
#[cfg(test)]
mod tests;
