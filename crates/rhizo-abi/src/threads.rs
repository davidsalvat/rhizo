//! Threads of a cell (spec `threads`): `clone`, `clone3`, the exit of one
//! thread, and futexes (design D3 and D5 of `add-threads`).

use super::{Context, Frames, Services, errno::*, timespec_ns};
use rhizo_cell::Cell;
use rhizo_cell::threads::{Thread, Wait};
use rhizo_hal::PhysicalMemory;
use rhizo_mm::paging::EntryFormat;

const CLONE_VM: u64 = 0x100;
const CLONE_FS: u64 = 0x200;
const CLONE_FILES: u64 = 0x400;
const CLONE_SIGHAND: u64 = 0x800;
const CLONE_THREAD: u64 = 0x1_0000;
const CLONE_SYSVSEM: u64 = 0x4_0000;
const CLONE_SETTLS: u64 = 0x8_0000;
const CLONE_PARENT_SETTID: u64 = 0x10_0000;
const CLONE_CHILD_CLEARTID: u64 = 0x20_0000;
const CLONE_DETACHED: u64 = 0x40_0000;
const CLONE_CHILD_SETTID: u64 = 0x100_0000;
/// What makes a clone a thread.
const THREAD: u64 = CLONE_VM | CLONE_SIGHAND | CLONE_THREAD;
/// Every flag a thread clone may carry.
const ALLOWED: u64 = THREAD
    | CLONE_FS
    | CLONE_FILES
    | CLONE_SYSVSEM
    | CLONE_SETTLS
    | CLONE_PARENT_SETTID
    | CLONE_CHILD_CLEARTID
    | CLONE_DETACHED
    | CLONE_CHILD_SETTID;
/// The first address above the lower half, where cells live.
const LOWER_HALF_END: u64 = 0x0000_8000_0000_0000;
/// The exit signal in the low byte of `clone`'s flags.
const CSIGNAL: u64 = 0xff;

/// The arguments of a `clone` or `clone3`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CloneArgs {
    pub flags: u64,
    /// New stack pointer (0: the caller's).
    pub sp: u64,
    pub tls: u64,
    pub parent_tid: u64,
    pub child_tid: u64,
}

/// Kernel memory a thread's record takes.
const THREAD_BYTES: u64 = size_of::<Thread>() as u64;

/// Creates a thread (spec `threads`, creating threads and refused clones);
/// returns its identifier or a negated `errno`.
pub(crate) fn clone<F: EntryFormat, M: PhysicalMemory, T, A: Frames, S: Services>(
    cell: &mut Cell,
    cx: &mut Context<'_, F, M, T, A, S>,
    args: CloneArgs,
) -> i64 {
    if args.flags & THREAD != THREAD {
        return -ENOSYS; // a new process: cells have none (design D5)
    }
    if args.flags & !ALLOWED != 0 {
        return -EINVAL;
    }
    // A thread pointer in the kernel half, or non-canonical, would fault
    // when loaded into the x86_64 FS base, in the kernel (Linux: `EPERM`).
    if args.flags & CLONE_SETTLS != 0 && args.tls >= LOWER_HALF_END {
        return -EPERM;
    }
    // Its record counts against the cell's memory (design D4 of
    // `add-cell-hierarchy`) until it exits.
    if !cx.store(THREAD_BYTES) {
        return -EAGAIN;
    }
    let mut regs = *cell.regs();
    cx.services.save_fp(&mut regs.fp);
    regs.gpr[0] = 0;
    if args.sp != 0 {
        regs.sp = args.sp;
    }
    if args.flags & CLONE_SETTLS != 0 {
        regs.tls = args.tls;
    }
    let clear = match args.flags & CLONE_CHILD_CLEARTID {
        0 => 0,
        _ => args.child_tid,
    };
    let Some(tid) = cell.threads.spawn(regs, clear) else {
        cx.frames.credit(THREAD_BYTES);
        return -EAGAIN;
    };
    // As on Linux, a bad address here does not undo the thread.
    for (flag, addr) in [
        (CLONE_PARENT_SETTID, args.parent_tid),
        (CLONE_CHILD_SETTID, args.child_tid),
    ] {
        if args.flags & flag != 0 {
            let _ = cx.write(addr, &tid.to_le_bytes());
        }
    }
    tid as i64
}

/// `clone(flags, stack, parent_tid, ...)`: the low byte of `flags` is the
/// exit signal, which threads ignore; `a3`, `a4` are `tls`, `child_tid` on
/// AArch64 and the other way round on x86_64.
pub(crate) fn clone_args(a: [u64; 6], x86_64: bool) -> CloneArgs {
    let (tls, child_tid) = if x86_64 { (a[4], a[3]) } else { (a[3], a[4]) };
    CloneArgs {
        flags: a[0] & !CSIGNAL,
        sp: a[1],
        tls,
        parent_tid: a[2],
        child_tid,
    }
}

/// Reads `struct clone_args` (`size` bytes at `addr`).
pub(crate) fn clone3_args<F: EntryFormat, M: PhysicalMemory, T, A, S>(
    cx: &mut Context<'_, F, M, T, A, S>,
    addr: u64,
    size: u64,
) -> Result<CloneArgs, i64> {
    // flags, pidfd, child_tid, parent_tid, exit_signal, stack, stack_size,
    // tls (64 bytes, version 0), then set_tid, set_tid_size, cgroup.
    if !(64..=4096).contains(&size) {
        return Err(-EINVAL);
    }
    let mut raw = [0u8; 88];
    let len = size.min(88) as usize;
    cx.read(addr, &mut raw[..len]).map_err(|_| -EFAULT)?;
    let w = |i: usize| u64::from_le_bytes(raw[i * 8..i * 8 + 8].try_into().unwrap_or_default());
    let (flags, stack, stack_size) = (w(0), w(5), w(6));
    // `pidfd` (word 1) is only used with `CLONE_PIDFD`, which is refused.
    if flags & CSIGNAL != 0 || w(8) != 0 || w(9) != 0 || w(10) != 0 {
        return Err(-EINVAL);
    }
    if stack == 0 && stack_size != 0 {
        return Err(-EINVAL);
    }
    Ok(CloneArgs {
        flags,
        sp: if stack == 0 {
            0
        } else {
            stack.wrapping_add(stack_size)
        },
        tls: w(7),
        parent_tid: w(3),
        child_tid: w(2),
    })
}

/// `exit` of a thread that is not the last: its clear-tid word is zeroed
/// and one waiter on it woken (what `pthread_join` waits for).
pub(crate) fn exit_thread<F: EntryFormat, M: PhysicalMemory, T, A: Frames, S>(
    cell: &mut Cell,
    cx: &mut Context<'_, F, M, T, A, S>,
) {
    let Some(gone) = cell.threads.exit_current() else {
        return;
    };
    cx.frames.credit(THREAD_BYTES);
    if gone.clear_tid != 0 && cx.write(gone.clear_tid, &0u32.to_le_bytes()).is_ok() {
        cell.threads.wake(gone.clear_tid, u32::MAX, 1);
    }
}

const FUTEX_WAIT: u64 = 0;
const FUTEX_WAKE: u64 = 1;
const FUTEX_WAIT_BITSET: u64 = 9;
const FUTEX_WAKE_BITSET: u64 = 10;
const FUTEX_PRIVATE_FLAG: u64 = 128;
const FUTEX_CLOCK_REALTIME: u64 = 256;
const MATCH_ANY: u32 = u32::MAX;

/// `futex(addr, op, val, timeout, addr2, val3)`: the result, or `None`
/// when the calling thread now waits.
pub(crate) fn futex<F: EntryFormat, M: PhysicalMemory, T, A, S: Services>(
    cell: &mut Cell,
    cx: &mut Context<'_, F, M, T, A, S>,
    a: [u64; 6],
) -> Option<i64> {
    let (addr, op, val, timeout, val3) = (a[0], a[1], a[2] as u32, a[3], a[5] as u32);
    let cmd = op & !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);
    if op & FUTEX_CLOCK_REALTIME != 0 && cmd != FUTEX_WAIT_BITSET {
        return Some(-ENOSYS); // as Linux
    }
    let bitset = match cmd {
        FUTEX_WAIT | FUTEX_WAKE => MATCH_ANY,
        FUTEX_WAIT_BITSET | FUTEX_WAKE_BITSET => val3,
        _ => return Some(-ENOSYS),
    };
    if bitset == 0 || !addr.is_multiple_of(4) {
        return Some(-EINVAL);
    }
    if cmd == FUTEX_WAKE || cmd == FUTEX_WAKE_BITSET {
        // Linux wakes one even when asked for none.
        let n = (val as i32).max(1) as u32;
        return Some(cell.threads.wake(addr, bitset, n) as i64);
    }
    let until = match timeout {
        0 => None,
        ts => {
            let value = match timespec_ns(cx, ts) {
                Ok(ns) => ns,
                Err(e) => return Some(e),
            };
            let now = cx.services.monotonic_ns();
            Some(if cmd == FUTEX_WAIT {
                now.saturating_add(value)
            } else if op & FUTEX_CLOCK_REALTIME != 0 {
                now.saturating_add(value.saturating_sub(cx.services.realtime_ns()))
            } else {
                value
            })
        }
    };
    let mut word = [0u8; 4];
    if cx.read(addr, &mut word).is_err() {
        return Some(-EFAULT);
    }
    if u32::from_le_bytes(word) != val {
        return Some(-EAGAIN);
    }
    if until.is_some_and(|t| t <= cx.services.monotonic_ns()) {
        return Some(-ETIMEDOUT);
    }
    cell.threads.block(Wait::Futex {
        addr,
        bitset,
        until,
    });
    None
}
