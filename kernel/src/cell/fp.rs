//! Lazy switching of the FP/SIMD registers between the threads of cells.

use super::*;

/// Who owns the CPU's FP/SIMD registers: thread `tid` of cell `cell`
/// (cell identifiers start at 1, and neither they nor thread identifiers
/// are reused, so a key never names another thread). Besides keys, the
/// owner can be [`FP_STALE`] or [`FP_CLEAN`].
pub(super) fn fp_key(cell: u32, tid: u32) -> u64 {
    ((cell as u64) << 32) | tid as u64
}

/// No thread owns the registers, and they may still hold the values of one
/// that ran (saved already, or moot): they are loaded or cleared before any
/// cell code runs.
pub(super) const FP_STALE: u64 = 0;
/// No thread owns the registers, and they hold the initial state
/// ([`CLEAN_FP`]): nothing of any thread or cell.
const FP_CLEAN: u64 = u64::MAX;
/// What the registers hold while a thread runs that has not used FP/SIMD
/// yet: a thread starts with it, and it comes from no thread or cell.
static CLEAN_FP: FpState = FpState::initial(user::MACHINE);

/// Gets this CPU's FP/SIMD registers ready for the thread `key`, whose
/// registers are `regs`, about to run (design D1 of `lazy-fp-switch`):
/// they hold its own state, or the initial state; never another thread's,
/// not even for cell code that runs speculatively before an FP/SIMD
/// trap is taken (LazyFP, CVE-2018-3665). A thread that has used FP/SIMD
/// gets its state at once; one that has not runs with FP/SIMD trapping.
/// The thread that owned the registers before has been saved already.
/// Returns whether the registers now hold the thread's state.
///
/// Whether FP/SIMD traps follows the owner: never while a thread owns the
/// registers, always while they are [`FP_CLEAN`] (either way while they
/// are [`FP_STALE`]), so the cases that need no change cost no write.
/// `fp_save` and `fp_load` never change it on either architecture, so a
/// load or save outside switching (a test reading the registers) keeps
/// this true.
#[inline(always)]
pub(super) fn fp_enter(key: u64, regs: &Registers) -> bool {
    // Only a thread that has used FP/SIMD can own the registers.
    let owner = fp_owner();
    match regs.fp_used != 0 {
        true if owner == key => true,
        false if owner == FP_CLEAN => false,
        _ => fp_switch(key, regs),
    }
}

/// [`fp_enter`] when the registers must change: loads the thread's state
/// (it has used FP/SIMD), or clears them for it, with FP/SIMD trapping.
#[inline(never)]
fn fp_switch(key: u64, regs: &Registers) -> bool {
    if regs.fp_used != 0 {
        user::fp_trap(false);
        user::fp_load(&regs.fp);
        set_fp_owner(key);
    } else {
        user::fp_load(&CLEAN_FP);
        user::fp_trap(true);
        set_fp_owner(FP_CLEAN);
    }
    regs.fp_used != 0
}

/// Saves the FP/SIMD registers, which thread `key` of `live` owns, into
/// that thread (if it has not exited meanwhile).
pub(super) fn fp_save_thread(live: &mut Live, key: u64) {
    if let Some(thread) = live.cell.threads.get_mut(key as u32) {
        user::fp_save(&mut thread.regs.fp);
    }
}

/// The first FP/SIMD instruction of the thread `key`, whose registers are
/// `regs`: the registers hold the initial state ([`fp_enter`]), so its own
/// state is loaded, and from now on it is switched eagerly.
pub(super) fn fp_first_use(key: u64, regs: &mut Registers) {
    debug_assert_eq!(fp_owner(), FP_CLEAN, "FP/SIMD trap with registers in use");
    regs.fp_used = 1;
    user::fp_trap(false);
    user::fp_load(&regs.fp);
    set_fp_owner(key);
}

/// The owner ([`fp_key`]) of each CPU's FP/SIMD registers, by CPU number.
static FP_OWNER: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// The CPU's FP/SIMD owner ([`fp_key`]).
pub(super) fn fp_owner() -> u64 {
    FP_OWNER[crate::arch::cpu::index()].load(Ordering::Relaxed)
}

pub(super) fn set_fp_owner(key: u64) {
    FP_OWNER[crate::arch::cpu::index()].store(key, Ordering::Relaxed);
}

/// Brings the FP/SIMD state of `live`'s threads up to date in their
/// registers, if one of them has it in this CPU; with `leaving`, the
/// CPU's copy stops belonging to it.
pub(super) fn sync_fp(live: &mut Live, leaving: bool) {
    let owner = fp_owner();
    if owner >> 32 == live.cell.id as u64 {
        if let Some(thread) = live.cell.threads.get_mut(owner as u32) {
            user::fp_save(&mut thread.regs.fp);
        }
        if leaving {
            set_fp_owner(FP_STALE);
        }
    }
}
