//! The Linux system-call interface of Rhizo cells (spec `linux-abi`).
//!
//! [`Abi::handle`] decodes the call from a cell's saved registers with the
//! Linux convention of its architecture, runs it against the cell's address
//! space and the kernel [`Services`], and writes the result (or the negated
//! `errno`) back. Everything goes through traits, so it is tested on the host.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod files;
mod threads;

use core::fmt;
use rhizo_cell::memory::{self, Pager};
use rhizo_cell::space::{self, Frames};
use rhizo_cell::threads::Wait;
use rhizo_cell::{Cell, FpState, MMAP_BASE, STACK_PAGES, STACK_TOP};
use rhizo_hal::{Console, PhysicalMemory, Tlb};
use rhizo_mm::paging::{EntryFormat, PageFlags, PageTable};
use rhizo_mm::{FRAME_SIZE, LARGE_PAGE_SIZE, align_up};

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
    pub const ENOTTY: i64 = 25;
    pub const ENOSYS: i64 = 38;
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
    Clone,
    Clone3,
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
            (Arch::Aarch64, 220) | (Arch::X86_64, 56) => Clone,
            (Arch::Aarch64, 435) | (Arch::X86_64, 435) => Clone3,
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

    const fn machine(self) -> &'static [u8] {
        match self {
            Arch::Aarch64 => b"aarch64",
            Arch::X86_64 => b"x86_64",
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
    /// Whether the cell may store `bytes` more of kernel-held data (file
    /// blocks, thread records) without eating into the memory the kernel
    /// keeps for itself.
    fn can_store(&mut self, bytes: u64) -> bool;
    /// Brings `fp`, the calling thread's FP/SIMD state, up to date when the
    /// CPU's registers hold a newer copy (the kernel switches them lazily).
    fn save_fp(&mut self, fp: &mut FpState) {
        let _ = fp;
    }
}

/// What the cell does after a system call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Return to the calling thread (the result is in its registers).
    Resume,
    /// Let other cells run first, then resume the cell.
    Yield,
    /// The calling thread stopped: it waits (a sleep or a futex, recorded
    /// in the cell's thread table) or it exited. Another thread of the cell
    /// runs, or the cell waits as its threads say.
    Block,
    /// The cell exited with this status (0–255).
    Exit(i32),
}

pub use rhizo_cell::memory::NoPager;

/// Everything a call needs to reach the cell's memory and the kernel.
pub struct Context<'a, F, M, T, A, S> {
    pub pt: PageTable<F>,
    pub mem: &'a mut M,
    pub tlb: &'a mut T,
    pub frames: &'a mut A,
    pub services: &'a mut S,
    /// The cell's files and descriptors.
    pub fs: &'a mut rhizo_fs::Fs,
    /// Brings in pages that are not mapped yet (lazy resume); [`NoPager`]
    /// otherwise.
    pub pager: &'a mut dyn Pager<F, M>,
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

/// The system-call handler.
#[derive(Debug)]
pub struct Abi {
    arch: Arch,
    /// Unimplemented call numbers already reported (one bit each, < 1024,
    /// beyond every Linux call number; larger values are reported each time).
    reported: [u64; 16],
}

/// `AT_FDCWD` as a system-call argument.
const AT_FDCWD: u64 = -100i64 as u64;
/// Bytes copied per step when moving data between a cell and the kernel.
const CHUNK: usize = 256;
/// Lowest address the stack region (with its guard page) may reach.
const STACK_FLOOR: u64 = STACK_TOP - (STACK_PAGES + 1) * FRAME_SIZE;
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
            Some(Sys::Writev) => writev(cx, a[0], a[1], a[2]),
            Some(Sys::Brk) => brk(cell, cx, a[0]),
            Some(Sys::Mmap) => mmap(cell, cx, a),
            Some(Sys::Munmap) => munmap(cx, a[0], a[1]),
            Some(Sys::Getpid) => cell.id as i64,
            Some(Sys::Gettid) => cell.threads.current().tid as i64,
            Some(Sys::SetTidAddress) => {
                let thread = cell.threads.current_mut();
                thread.clear_tid = a[0];
                thread.tid as i64
            }
            Some(Sys::Uname) => uname(cx, self.arch, a[0]),
            Some(Sys::ClockGettime) => clock_gettime(cx, a[0], a[1]),
            Some(Sys::Getrandom) => getrandom(cx, a[0], a[1]),
            // Signals are not delivered yet: accept, report no previous state.
            Some(Sys::RtSigaction) => zero_out(cx, a[2], 32),
            Some(Sys::RtSigprocmask) => zero_out(cx, a[2], a[3].min(128)),
            Some(Sys::Ioctl) => -ENOTTY,
            Some(Sys::ArchPrctl) => arch_prctl(cell, cx, a[0], a[1]),
            Some(Sys::Mprotect) => mprotect(cell, cx, a[0], a[1], a[2]),
            // Robust futex lists are never walked (no thread dies holding a
            // lock without its cell ending too: there are no signals).
            Some(Sys::SetRobustList) => 0,
            // Declined on purpose (as on Linux built without it): libc falls
            // back, so it is not reported as missing.
            Some(Sys::Rseq) => -ENOSYS,
            Some(Sys::Prlimit64) => rlimit(cx, a[1], a[3]),
            Some(Sys::Getrlimit) => rlimit(cx, a[0], a[1]),
            // No symbolic links exist.
            Some(Sys::Readlink | Sys::Readlinkat) => -ENOENT,
            // Cells run as root of their own world: uid, gid 0; no parent.
            Some(Sys::Ids | Sys::Getppid) => 0,
            Some(Sys::Sigaltstack | Sys::Madvise) => 0,
            Some(Sys::Futex) => match threads::futex(cell, cx, a) {
                Some(result) => result,
                None => return Outcome::Block,
            },
            Some(Sys::Fstat) => files::fstat(cx, self.arch, a[0], a[1]),
            Some(Sys::Newfstatat) => files::newfstatat(cx, self.arch, a[0], a[1], a[2], a[3]),
            Some(Sys::Stat | Sys::Lstat) => {
                files::newfstatat(cx, self.arch, AT_FDCWD, a[0], a[1], 0)
            }
            Some(Sys::Openat) => files::openat(cx, self.arch, a[0], a[1], a[2]),
            Some(Sys::Open) => files::openat(cx, self.arch, AT_FDCWD, a[0], a[1]),
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
                Ok(_) if a[0] == a[1] => a[1] as i64,
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
            // Without descriptors, `select` is a sleep (how MicroPython's
            // `time.sleep` waits); descriptors are not supported yet.
            Some(Sys::Select | Sys::Pselect6) if a[0] != 0 => -ENOSYS,
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
        // The result goes in x0 / rax, which is gpr[0] on both.
        cell.regs_mut().gpr[0] = result as u64;
        Outcome::Resume
    }

    fn report_unimplemented(&mut self, cell: u32, nr: u64, out: &mut impl Services) {
        if let Some(word) = self.reported.get_mut((nr / 64) as usize) {
            if *word & (1 << (nr % 64)) != 0 {
                return;
            }
            *word |= 1 << (nr % 64);
        }
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

fn writev<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    iov: u64,
    count: u64,
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
        let n = files::write(cx, fd, base, len, None);
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

fn brk<F: EntryFormat, M: PhysicalMemory, T: Tlb, A: Frames, S>(
    cell: &mut Cell,
    cx: &mut Context<'_, F, M, T, A, S>,
    want: u64,
) -> i64 {
    if want < cell.brk_start || want > MMAP_BASE {
        return cell.brk as i64;
    }
    let (old_end, new_end) = (align_up(cell.brk, FRAME_SIZE), align_up(want, FRAME_SIZE));
    let ok = if new_end > old_end {
        space::map_anonymous(
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

fn mmap<F: EntryFormat, M: PhysicalMemory, T: Tlb, A: Frames, S>(
    cell: &mut Cell,
    cx: &mut Context<'_, F, M, T, A, S>,
    a: [u64; 6],
) -> i64 {
    let (len, prot, flags) = (a[1], a[2], a[3]);
    if len == 0 || flags & (MAP_ANONYMOUS | MAP_PRIVATE) != (MAP_ANONYMOUS | MAP_PRIVATE) {
        return -EINVAL;
    }
    if flags & MAP_FIXED != 0 {
        return -EINVAL;
    }
    if prot & PROT_WRITE != 0 && prot & PROT_EXEC != 0 {
        return -EACCES; // cells are W^X
    }
    let Some(len) = len
        .checked_add(FRAME_SIZE - 1)
        .map(|l| l & !(FRAME_SIZE - 1))
    else {
        return -ENOMEM;
    };
    let align = if len >= LARGE_PAGE_SIZE {
        LARGE_PAGE_SIZE
    } else {
        FRAME_SIZE
    };
    let addr = align_up(cell.mmap_next, align);
    if addr.checked_add(len).is_none_or(|end| end > STACK_FLOOR) {
        return -ENOMEM;
    }
    if prot != 0 {
        let mut pf = PageFlags::USER;
        if prot & PROT_WRITE != 0 {
            pf = pf | PageFlags::WRITABLE;
        }
        if prot & PROT_EXEC == 0 {
            pf = pf | PageFlags::NO_EXECUTE;
        }
        if space::map_anonymous(&cx.pt, cx.mem, cx.tlb, cx.frames, addr, len, pf).is_err() {
            return -ENOMEM;
        }
    }
    cell.mmap_next = addr + len;
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
    match space::unmap(&cx.pt, cx.mem, cx.tlb, cx.frames, addr, end - addr) {
        Ok(()) => {
            cx.pager.discard(addr, end - addr);
            0
        }
        Err(_) => -EINVAL,
    }
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
    if prot & PROT_WRITE != 0 && prot & PROT_EXEC != 0 {
        return -EACCES; // cells are W^X
    }
    if prot == 0 {
        return -EINVAL; // inaccessible pages are not supported yet
    }
    let Some(end) = addr.checked_add(len).map(|e| align_up(e, FRAME_SIZE)) else {
        return -ENOMEM;
    };
    let mut flags = PageFlags::USER;
    if prot & PROT_WRITE != 0 {
        flags = flags | PageFlags::WRITABLE;
    }
    if prot & PROT_EXEC == 0 {
        flags = flags | PageFlags::NO_EXECUTE;
    }
    // Pages not in memory yet (lazy resume) are brought in first; holes
    // below the mapping cursor are reserved (`PROT_NONE`) addresses, backed
    // now (spec `linux-abi`, reserved mappings; design D6 of `add-threads`).
    let reserved = MMAP_BASE..cell.mmap_next;
    let present = |cx: &mut Context<'_, F, M, T, A, S>, at: u64| {
        cx.pt.translate(cx.mem, at).is_some() || cx.pager.page_in(&cx.pt, cx.mem, at, false)
    };
    let mut at = addr;
    while at < end {
        if present(cx, at) || !reserved.contains(&at) {
            at += FRAME_SIZE;
            continue;
        }
        let hole = at;
        while at < end && reserved.contains(&at) && !present(cx, at) {
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

/// `getrlimit`/`prlimit64` (no new limit is enforced): an 8 MiB stack, 1024
/// files, everything else unlimited.
fn rlimit<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    resource: u64,
    old: u64,
) -> i64 {
    const INFINITY: u64 = u64::MAX;
    let (cur, max) = match resource {
        3 => (8 << 20, INFINITY), // RLIMIT_STACK
        7 => (1024, 1024),        // RLIMIT_NOFILE
        0..=15 => (INFINITY, INFINITY),
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
        ARCH_SET_FS => {
            cell.regs_mut().tls = addr;
            0
        }
        ARCH_GET_FS => put(cx, addr, &cell.regs().tls.to_le_bytes()),
        _ => -EINVAL,
    }
}

#[cfg(test)]
mod tests;
