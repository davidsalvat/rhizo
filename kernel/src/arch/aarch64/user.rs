//! Running cell code at EL0: entry, return on `SVC` or an exception, and
//! the lower-half table switch (design decision D3). The vectors and the
//! entry code are in `cpu.rs`.

use super::cpu::{EMPTY_ROOT, USER_ROOT, index};
use core::arch::asm;
use core::sync::atomic::{AtomicBool, Ordering};
use rhizo_cell::{FpState, Registers};
use rhizo_core::exception::{self, Aarch64Exception};
use rhizo_core::smp::MAX_CPUS;
use rhizo_hal::Console;

/// ELF machine of cell executables.
pub const MACHINE: rhizo_cell::elf::Machine = rhizo_cell::elf::Machine::Aarch64;
/// System-call convention of cells.
pub const ABI: rhizo_abi::Arch = rhizo_abi::Arch::Aarch64;
/// Whether a cell's root table must carry the kernel half: no, the kernel
/// lives in `TTBR1_EL1` and the cell's table goes in `TTBR0_EL1`.
pub const CELL_ROOT_HAS_KERNEL_HALF: bool = false;
/// Whether a TLB flush reaches every CPU: yes, the kernel only uses the
/// inner-shareable (broadcast) `TLBI` forms for cell translations.
pub const TLB_FLUSH_IS_BROADCAST: bool = true;

unsafe extern "C" {
    fn rhizo_enter_cell(regs: *mut Registers) -> u64;
}

/// Why a cell stopped running.
#[derive(Debug, Clone, Copy)]
pub enum Trap {
    /// `SVC #0`: the system call is in the saved registers.
    Syscall,
    /// An interrupt (the end of its time slice): the cell can be resumed.
    Interrupt,
    /// The sampling interrupt of the performance counters (spec
    /// `profiling`): `pc` of the saved registers is the sample; resume.
    Sample,
    /// A first FP/SIMD instruction while the registers belong to another
    /// cell (see [`fp_trap`]): swap the state and resume.
    FpUnavailable,
    /// Any other exception: the cell cannot continue.
    Fault(UserFault),
}

/// An exception raised by cell code.
#[derive(Debug, Clone, Copy)]
pub struct UserFault(Aarch64Exception);

impl UserFault {
    /// The address of an access to a page that is not mapped (translation
    /// fault, data or instruction abort from EL0), if that is what it was.
    pub fn missing_page(&self) -> Option<u64> {
        let ec = (self.0.esr >> 26) & 0x3f;
        let translation = (self.0.esr & 0x3f) >> 2 == 0b0001;
        (matches!(ec, 0x20 | 0x24) && translation).then_some(self.0.far)
    }

    /// The address of a write refused by permissions (a data abort from EL0
    /// with a permission fault on a write), if that is what it was: possibly
    /// a copy-on-write page.
    pub fn write_denied(&self) -> Option<u64> {
        let esr = self.0.esr;
        let permission = (esr & 0x3f) >> 2 == 0b0011;
        let write = esr & (1 << 6) != 0;
        ((esr >> 26) & 0x3f == 0x24 && permission && write).then_some(self.0.far)
    }

    /// Answers the cell's read of an ID register (`MRS Xt, MIDR_EL1`...,
    /// which EL0 cannot do; `HWCAP_CPUID` tells programs they may, as on
    /// Linux): the value goes to `regs`, which then resume after the
    /// instruction. `insn` reads the instruction at an address of the cell.
    /// Returns whether the fault was such a read.
    pub fn emulate(&self, regs: &mut Registers, insn: impl FnOnce(u64) -> Option<u32>) -> bool {
        let Some(read) = exception::IdRegisterRead::decode(self.0.esr, || insn(self.0.elr)) else {
            return false;
        };
        let (midr, revidr): (u64, u64);
        // SAFETY: reading two ID registers at EL1 has no side effect.
        unsafe {
            asm!("mrs {}, midr_el1", "mrs {}, revidr_el1", out(reg) midr, out(reg) revidr, options(nomem, nostack));
        }
        if let Some(rt) = regs.gpr.get_mut(read.rt) {
            *rt = read.value(midr, revidr);
        }
        regs.pc += 4;
        true
    }

    /// Prints `cell N killed: <report>`.
    pub fn report(&self, console: &mut impl Console, cell: u32) {
        exception::report_cell_aarch64(console, cell, &self.0);
    }
}

// FP/SIMD state of a cell: q0-q31 at 0-511, FPCR at 512, FPSR at 520.
core::arch::global_asm!(
    r#"
    .arch_extension fp
    .arch_extension simd
    .text
    .global rhizo_fp_save
rhizo_fp_save:
    stp q0, q1, [x0, #0]
    stp q2, q3, [x0, #32]
    stp q4, q5, [x0, #64]
    stp q6, q7, [x0, #96]
    stp q8, q9, [x0, #128]
    stp q10, q11, [x0, #160]
    stp q12, q13, [x0, #192]
    stp q14, q15, [x0, #224]
    stp q16, q17, [x0, #256]
    stp q18, q19, [x0, #288]
    stp q20, q21, [x0, #320]
    stp q22, q23, [x0, #352]
    stp q24, q25, [x0, #384]
    stp q26, q27, [x0, #416]
    stp q28, q29, [x0, #448]
    stp q30, q31, [x0, #480]
    mrs x1, fpcr
    mrs x2, fpsr
    str x1, [x0, #512]
    str x2, [x0, #520]
    ret

    .global rhizo_fp_load
rhizo_fp_load:
    ldp q0, q1, [x0, #0]
    ldp q2, q3, [x0, #32]
    ldp q4, q5, [x0, #64]
    ldp q6, q7, [x0, #96]
    ldp q8, q9, [x0, #128]
    ldp q10, q11, [x0, #160]
    ldp q12, q13, [x0, #192]
    ldp q14, q15, [x0, #224]
    ldp q16, q17, [x0, #256]
    ldp q18, q19, [x0, #288]
    ldp q20, q21, [x0, #320]
    ldp q22, q23, [x0, #352]
    ldp q24, q25, [x0, #384]
    ldp q26, q27, [x0, #416]
    ldp q28, q29, [x0, #448]
    ldp q30, q31, [x0, #480]
    ldr x1, [x0, #512]
    ldr x2, [x0, #520]
    msr fpcr, x1
    msr fpsr, x2
    ret
    "#
);

unsafe extern "C" {
    fn rhizo_fp_save(state: *mut FpState);
    fn rhizo_fp_load(state: *const FpState);
}

/// Whether FP/SIMD use by cell code traps on each CPU (`CPACR_EL1.FPEN` =
/// 0b01: EL0 traps, EL1 does not, so the kernel can still save and load).
static FP_TRAPS: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];

/// Makes FP/SIMD use by cell code trap (`true`) or not on this CPU, writing
/// `CPACR_EL1` only when that changes.
pub fn fp_trap(enable: bool) {
    if FP_TRAPS[index()].swap(enable, Ordering::Relaxed) == enable {
        return;
    }
    let fpen: u64 = if enable { 0b01 } else { 0b11 };
    // SAFETY: CPACR_EL1.FPEN only decides whether FP/SIMD traps; EL1 access
    // stays enabled either way.
    unsafe {
        asm!(
            "mrs {t}, cpacr_el1",
            "bic {t}, {t}, #(3 << 20)",
            "orr {t}, {t}, {f}",
            "msr cpacr_el1, {t}",
            "isb",
            t = out(reg) _,
            f = in(reg) fpen << 20,
            options(nostack)
        );
    }
}

/// Saves the FP/SIMD registers (the kernel never uses them, so they hold the
/// state of the cell that last ran).
pub fn fp_save(state: &mut FpState) {
    // SAFETY: `state` is 528 writable bytes; FP access is enabled at EL1.
    unsafe { rhizo_fp_save(state) }
}

/// Loads the FP/SIMD registers for the cell about to run.
pub fn fp_load(state: &FpState) {
    // SAFETY: `state` is 528 readable bytes; FP access is enabled at EL1.
    unsafe { rhizo_fp_load(state) }
}

/// Whether the CPU has Privileged Access Never (`ID_AA64MMFR1_EL1.PAN`).
pub fn has_pan() -> bool {
    let mmfr1: u64;
    // SAFETY: reading an ID register has no side effects.
    unsafe { asm!("mrs {}, id_aa64mmfr1_el1", out(reg) mmfr1, options(nomem, nostack)) };
    (mmfr1 >> 20) & 0xf != 0
}

/// Turns on Privileged Access Never where the CPU has it (design D1 of
/// `add-user-access-guards`): EL1 loads and stores to pages EL0 can reach
/// fault. `SCTLR_EL1.SPAN` cleared sets `PSTATE.PAN` on every exception
/// taken to EL1, so it is back on after each return from a cell. (EL1
/// never executes a cell's page: they are mapped `PXN`.)
fn enable_pan() {
    if !has_pan() {
        return;
    }
    // SAFETY: the kernel never loads from or stores to a lower-half address:
    // cell memory is reached through the direct map (`sync_code` cleans
    // caches by direct-map address too), and the entry code touches only
    // per-CPU data, kernel stacks and the `Registers` in the kernel heap.
    // `s3_0_c4_c2_3` is `PAN` (bit 22).
    unsafe {
        asm!(
            "mrs {t}, sctlr_el1",
            "bic {t}, {t}, #(1 << 23)",
            "msr sctlr_el1, {t}",
            "mov {t}, #(1 << 22)",
            "msr s3_0_c4_c2_3, {t}",
            "isb",
            t = out(reg) _,
            options(nostack)
        );
    }
}

/// Whether `PSTATE.PAN` is set on this CPU now.
#[cfg(feature = "ktest")]
pub fn pan_on() -> bool {
    let pan: u64;
    // SAFETY: reading PAN has no side effects (only called when it exists).
    unsafe { asm!("mrs {}, s3_0_c4_c2_3", out(reg) pan, options(nomem, nostack)) };
    pan & (1 << 22) != 0
}

/// Enables FP/SIMD on this CPU for cells (and for the save and load
/// above), and PAN. The boot CPU calls it through [`init_tags`]; the
/// others once at bring-up, with the boot CPU's `TCR_EL1` (ASID size)
/// already copied.
pub fn init_cpu() {
    enable_pan();
    // SAFETY: CPACR_EL1.FPEN = 0b11 stops trapping FP/SIMD at EL0 and EL1;
    // the kernel is built without FP instructions, so only cells and the
    // explicit save/load use them.
    unsafe {
        asm!(
            "mrs {t}, cpacr_el1",
            "orr {t}, {t}, #(3 << 20)",
            "msr cpacr_el1, {t}",
            "isb",
            t = out(reg) _,
            options(nostack)
        );
    }
    USER_ROOT[index()].store(EMPTY_ROOT.load(Ordering::Relaxed), Ordering::Relaxed);
}

/// Enables FP/SIMD for cells (and for the save and load above), and 16-bit
/// ASIDs when the CPU has them; returns the highest tag cells may use
/// (65535, or 255 with 8-bit ASIDs). Call once, on the boot CPU, before the
/// others start.
pub fn init_tags() -> u16 {
    init_cpu();
    let mmfr0: u64;
    // SAFETY: reading an ID register has no side effects.
    unsafe { asm!("mrs {}, id_aa64mmfr0_el1", out(reg) mmfr0, options(nomem, nostack)) };
    if (mmfr0 >> 4) & 0xf != 0b0010 {
        return 255;
    }
    // SAFETY: TCR_EL1.AS selects 16-bit ASIDs; no cell translation is cached
    // yet (only ASID 0 was ever used, and it is flushed with everything).
    unsafe {
        asm!(
            "mrs {t}, tcr_el1",
            "orr {t}, {t}, #(1 << 36)",
            "msr tcr_el1, {t}",
            "isb",
            "tlbi vmalle1",
            "dsb nsh",
            "isb",
            t = out(reg) _,
            options(nostack)
        );
    }
    u16::MAX
}

/// Loads `root` with ASID `tag` in this CPU's `TTBR0_EL1`. A tagged switch
/// needs no TLB maintenance (design D6); tag 0 is shared by the empty table
/// and untagged cells, so switching to it drops what ASID 0 had cached.
fn set_user_root(root: u64, tag: u16) {
    let ttbr = root | ((tag as u64) << 48);
    if USER_ROOT[index()].swap(ttbr, Ordering::Relaxed) == ttbr {
        return;
    }
    // SAFETY: TTBR0 maps only the lower half, which the kernel never uses;
    // cell pages are non-global, so their translations are tagged.
    unsafe {
        asm!("dsb ishst", "msr ttbr0_el1, {}", "isb", in(reg) ttbr, options(nostack));
        if tag == 0 {
            asm!("tlbi aside1, xzr", "dsb ish", "isb", options(nostack));
        }
    }
}

/// Runs the cell whose lower-half table is `root`, tagged `tag` (0 =
/// untagged), from `regs` until it makes a system call, is interrupted or
/// faults; `regs` then holds its state at that point.
///
/// `flush` drops the tag's translations before its tables are loaded (on
/// every CPU: the flush is broadcast), so nothing a previous holder of the
/// tag left can be used (design D6 of `add-scheduler`).
pub fn enter_user(root: u64, tag: u16, flush: bool, regs: &mut Registers) -> Trap {
    if flush && tag != 0 {
        // SAFETY: invalidating one ASID only drops cached translations; the
        // tag is not loaded in TTBR0 (its previous holder ended).
        unsafe {
            asm!(
                "dsb ishst",
                "tlbi aside1is, {}",
                "dsb ish",
                "isb",
                in(reg) (tag as u64) << 48,
                options(nostack)
            );
        }
    }
    set_user_root(root, tag);
    // SAFETY: `regs` is a valid, 16-byte aligned frame that lives across the
    // call; the entry code drops to EL0 with only the NZCV flags of the
    // saved state, and the EL0 vectors restore the kernel stacks and
    // callee-saved registers before returning here.
    let vector = unsafe { rhizo_enter_cell(regs) };
    let (esr, far): (u64, u64);
    // SAFETY: reading the syndrome registers of the exception just taken.
    unsafe {
        asm!("mrs {}, esr_el1", "mrs {}, far_el1", out(reg) esr, out(reg) far, options(nomem, nostack));
    }
    if vector == 8 && (esr >> 26) & 0x3f == 0x15 {
        return Trap::Syscall;
    }
    if vector == 8 && (esr >> 26) & 0x3f == 0x07 {
        return Trap::FpUnavailable;
    }
    if vector == 9 {
        if super::pmu::is_sample(super::irq::acknowledge()) {
            super::pmu::rearm();
            return Trap::Sample;
        }
        return Trap::Interrupt;
    }
    Trap::Fault(UserFault(Aarch64Exception {
        esr,
        far,
        elr: regs.pc,
        stack_guard: (0, 0),
    }))
}

/// Drops the translations of a cell that keeps running (its permissions
/// shrank, as after a fork).
pub fn flush_user(root: u64, tag: u16) {
    release_user(root, tag);
}

/// Points this CPU's `TTBR0_EL1` back at the empty table, keeping the
/// cell's translations (another CPU may run it next; design D4 of
/// `add-smp-scheduling`). Nothing is flushed: the empty table maps nothing,
/// and entering an untagged cell drops ASID 0 anyway.
pub fn leave_user() {
    let empty = EMPTY_ROOT.load(Ordering::Relaxed);
    if USER_ROOT[index()].swap(empty, Ordering::Relaxed) != empty {
        // SAFETY: TTBR0 maps only the lower half, which the kernel never
        // uses; the empty table maps nothing.
        unsafe { asm!("msr ttbr0_el1, {}", "isb", in(reg) empty, options(nostack)) };
    }
}

/// Points `TTBR0_EL1` back at the empty table and drops every translation
/// of the cell tagged `tag`: call before freeing its tables and its tag.
pub fn release_user(_root: u64, tag: u16) {
    set_user_root(EMPTY_ROOT.load(Ordering::Relaxed), 0);
    if tag != 0 {
        // SAFETY: invalidating one ASID only drops cached translations.
        unsafe {
            asm!(
                "dsb ishst",
                "tlbi aside1is, {}",
                "dsb ish",
                "isb",
                in(reg) (tag as u64) << 48,
                options(nostack)
            );
        }
    }
}

/// Makes code written through the data side (a loaded cell image at the
/// direct-map address `virt`) visible to instruction fetch.
pub fn sync_code(virt: u64, len: u64) {
    let ctr: u64;
    // SAFETY: reading the cache type register has no side effects.
    unsafe { asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack)) };
    let line = 4 << ((ctr >> 16) & 0xf); // DminLine: log2 of words
    let mut at = virt & !(line - 1);
    while at < virt + len {
        // SAFETY: cleaning a cache line of mapped memory has no other effect.
        unsafe { asm!("dc cvau, {}", in(reg) at, options(nostack)) };
        at += line;
    }
    // SAFETY: invalidating the instruction cache only drops cached lines.
    unsafe { asm!("dsb ish", "ic ialluis", "dsb ish", "isb", options(nostack)) };
}

/// Loads the lower-half table `root` in `TTBR0_EL1`, untagged: for the
/// self-checks that touch a user page from the kernel.
#[cfg(any(feature = "ktest-user-read", feature = "ktest-user-exec"))]
pub fn load_tables(root: u64) {
    set_user_root(root, 0);
}
