//! The Linux system-call interface of Rhizo cells (spec `linux-abi`).
//!
//! [`Abi::handle`] decodes the call from a cell's saved registers with the
//! Linux convention of its architecture, runs it against the cell's address
//! space and the kernel [`Services`], and writes the result (or the negated
//! `errno`) back. Everything goes through traits, so it is tested on the host.

#![cfg_attr(not(test), no_std)]

use core::fmt;
use rhizo_cell::memory::{self, Pager};
use rhizo_cell::space::{self, Frames};
use rhizo_cell::{Cell, MMAP_BASE, STACK_PAGES, STACK_TOP};
use rhizo_hal::{Console, PhysicalMemory, Tlb};
use rhizo_mm::paging::{EntryFormat, PageFlags, PageTable};
use rhizo_mm::{FRAME_SIZE, LARGE_PAGE_SIZE, align_up};

/// Linux `errno` values used here.
pub mod errno {
    pub const EBADF: i64 = 9;
    pub const ENOMEM: i64 = 12;
    pub const EACCES: i64 = 13;
    pub const EFAULT: i64 = 14;
    pub const EINVAL: i64 = 22;
    pub const ENOTTY: i64 = 25;
    pub const ENOSYS: i64 = 38;
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
            (Arch::Aarch64, 101) | (Arch::X86_64, 35) => Nanosleep,
            (Arch::Aarch64, 115) | (Arch::X86_64, 230) => ClockNanosleep,
            (Arch::X86_64, 158) => ArchPrctl,
            _ => return None,
        })
    }

    /// (number, arguments) from the saved registers.
    fn call(self, cell: &Cell) -> (u64, [u64; 6]) {
        let r = &cell.regs.gpr;
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
}

/// What the cell does after a system call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Return to the cell (the result is in its registers).
    Resume,
    /// Let other cells run first, then resume this one.
    Yield,
    /// Resume the cell no earlier than this monotonic time (nanoseconds).
    Sleep(u64),
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
            Some(Sys::Exit | Sys::ExitGroup) => {
                let status = (a[0] & 0xff) as i32;
                cell.exit_status = Some(status);
                return Outcome::Exit(status);
            }
            Some(Sys::Write) => write(cx, a[0], &[(a[1], a[2])]),
            Some(Sys::Writev) => writev(cx, a[0], a[1], a[2]),
            Some(Sys::Brk) => brk(cell, cx, a[0]),
            Some(Sys::Mmap) => mmap(cell, cx, a),
            Some(Sys::Munmap) => munmap(cx, a[0], a[1]),
            Some(Sys::Getpid | Sys::Gettid | Sys::SetTidAddress) => cell.id as i64,
            Some(Sys::Uname) => uname(cx, self.arch, a[0]),
            Some(Sys::ClockGettime) => clock_gettime(cx, a[0], a[1]),
            Some(Sys::Getrandom) => getrandom(cx, a[0], a[1]),
            // Signals are not delivered yet: accept, report no previous state.
            Some(Sys::RtSigaction) => zero_out(cx, a[2], 32),
            Some(Sys::RtSigprocmask) => zero_out(cx, a[2], a[3].min(128)),
            Some(Sys::Ioctl) => -ENOTTY,
            Some(Sys::ArchPrctl) => arch_prctl(cell, cx, a[0], a[1]),
            Some(Sys::SchedYield) => {
                cell.regs.gpr[0] = 0;
                return Outcome::Yield;
            }
            Some(Sys::Nanosleep) => match sleep_until(cx, 1, 0, a[0]) {
                Ok(until) => {
                    cell.regs.gpr[0] = 0;
                    return Outcome::Sleep(until);
                }
                Err(e) => e,
            },
            Some(Sys::ClockNanosleep) => match sleep_until(cx, a[0], a[1], a[2]) {
                Ok(until) => {
                    cell.regs.gpr[0] = 0;
                    return Outcome::Sleep(until);
                }
                Err(e) => e,
            },
            None => {
                self.report_unimplemented(cell.id, nr, cx.services);
                -ENOSYS
            }
        };
        // The result goes in x0 / rax, which is gpr[0] on both.
        cell.regs.gpr[0] = result as u64;
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

fn write<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cx: &mut Context<'_, F, M, T, A, S>,
    fd: u64,
    iov: &[(u64, u64)],
) -> i64 {
    if fd != 1 && fd != 2 {
        return -EBADF;
    }
    let mut done = 0u64;
    for &(base, len) in iov {
        let mut off = 0;
        while off < len {
            let n = (len - off).min(CHUNK as u64) as usize;
            let mut buf = [0u8; CHUNK];
            if cx.read(base.wrapping_add(off), &mut buf[..n]).is_err() {
                return if done == 0 { -EFAULT } else { done as i64 };
            }
            cx.services.write_bytes(&buf[..n]);
            off += n as u64;
            done += n as u64;
        }
    }
    done as i64
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
        let n = write(cx, fd, &[(base, len)]);
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
        Ok(()) => 0,
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
    let mut ts = [0u8; 16];
    cx.read(addr, &mut ts).map_err(|_| -EFAULT)?;
    let sec = i64::from_le_bytes(ts[..8].try_into().unwrap_or_default());
    let nsec = i64::from_le_bytes(ts[8..].try_into().unwrap_or_default());
    if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
        return Err(-EINVAL);
    }
    let value = (sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(nsec as u64);
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
            cell.regs.tls = addr;
            0
        }
        ARCH_GET_FS => put(cx, addr, &cell.regs.tls.to_le_bytes()),
        _ => -EINVAL,
    }
}

#[cfg(test)]
mod tests;
