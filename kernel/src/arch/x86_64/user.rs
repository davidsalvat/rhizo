//! Running cell code in ring 3: entry, return on `SYSCALL` or an exception,
//! and the CR3 switch (design decision D4). The entry code is in `cpu.rs`.

use super::cpu::{index, read_cr2, wrmsr};
use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use rhizo_cell::{FpState, Registers};
use rhizo_core::exception::{self, ExceptionInfo};
use rhizo_core::smp::MAX_CPUS;
use rhizo_hal::Console;

const FS_BASE: u32 = 0xC000_0100;

/// ELF machine of cell executables.
pub const MACHINE: rhizo_cell::elf::Machine = rhizo_cell::elf::Machine::X86_64;
/// System-call convention of cells.
pub const ABI: rhizo_abi::Arch = rhizo_abi::Arch::X86_64;
/// Whether a cell's root table must carry the kernel half: yes, CR3 holds a
/// single hierarchy for both halves.
pub const CELL_ROOT_HAS_KERNEL_HALF: bool = true;
/// Whether a TLB flush reaches every CPU: no, `INVLPG` and CR3 loads act
/// on this CPU only.
pub const TLB_FLUSH_IS_BROADCAST: bool = false;

unsafe extern "C" {
    fn rhizo_enter_cell(regs: *mut Registers, sysret: u64) -> u64;
    fn rhizo_enter_cell_kpti(regs: *mut Registers, sysret: u64) -> u64;
}

/// Why a cell stopped running.
#[derive(Debug, Clone, Copy)]
pub enum Trap {
    /// `SYSCALL`: the system call is in the saved registers.
    Syscall,
    /// An interrupt (the end of its time slice): the cell can be resumed.
    Interrupt,
    /// The sampling interrupt of the performance counters (spec
    /// `profiling`): `pc` of the saved registers is the sample; resume.
    Sample,
    /// A first FP/SIMD instruction while the registers belong to another
    /// cell (`#NM`, see [`fp_trap`]): swap the state and resume.
    FpUnavailable,
    /// An exception: the cell cannot continue.
    Fault(UserFault),
}

/// An exception raised by cell code.
#[derive(Debug, Clone, Copy)]
pub struct UserFault(ExceptionInfo);

impl UserFault {
    /// The address of an access to a page that is not mapped (a page fault
    /// with the present bit clear), if that is what it was.
    pub fn missing_page(&self) -> Option<u64> {
        let info = &self.0;
        (info.vector == exception::PAGE_FAULT && info.error_code & 1 == 0)
            .then_some(info.fault_address)
            .flatten()
    }

    /// The address of a write refused by permissions (a page fault with the
    /// present and write bits set), if that is what it was: possibly a
    /// copy-on-write page.
    pub fn write_denied(&self) -> Option<u64> {
        let info = &self.0;
        (info.vector == exception::PAGE_FAULT && info.error_code & 0b11 == 0b11)
            .then_some(info.fault_address)
            .flatten()
    }

    /// Whether the kernel answered the fault by emulating the instruction
    /// (the AArch64 ID registers): never on x86_64, where cell code reads
    /// what it needs of the CPU itself (`CPUID`).
    #[allow(
        clippy::unused_self,
        reason = "the interface of AArch64's, which decodes the fault"
    )]
    pub fn emulate(&self, _regs: &mut Registers, _insn: impl FnOnce(u64) -> Option<u32>) -> bool {
        false
    }

    /// Prints `cell N killed: <report>`.
    pub fn report(&self, console: &mut impl Console, cell: u32) {
        exception::report_cell_exception(console, cell, &self.0);
    }
}

/// CR3 value (root and PCID) when a cell last ran on each CPU, and whether
/// PCIDs are on.
static USER_CR3: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// Whether each CPU's `SYSCALL` goes to the entry with KPTI (set as it
/// enters a cell after [`set_kpti`]; off the path of every other entry).
static KPTI_ENTRY: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
static PCID: AtomicBool = AtomicBool::new(false);
/// FS base last written on each CPU (skipping the write when it does not
/// change).
static LAST_FS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(u64::MAX) }; MAX_CPUS];
/// CR3 bit 63 with PCIDs on: keep the translations of the new PCID.
const NO_FLUSH: u64 = 1 << 63;

/// Whether `CR0.TS` is set on each CPU: the next FP/SIMD instruction
/// raises `#NM`.
static FP_TRAPS: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];

/// Makes FP/SIMD use trap (`CR0.TS`, `#NM`) or not on this CPU, writing CR0
/// only when that changes.
pub fn fp_trap(enable: bool) {
    if FP_TRAPS[index()].swap(enable, Ordering::Relaxed) == enable {
        return;
    }
    // SAFETY: CR0.TS only makes FP/SIMD instructions raise #NM; the kernel
    // uses none except `fp_save`/`fp_load`, which clear it first.
    unsafe {
        if enable {
            asm!("mov {t}, cr0", "or {t}, 8", "mov cr0, {t}", t = out(reg) _, options(nostack));
        } else {
            asm!("clts", options(nostack));
        }
    }
}

/// Saves the FP/SIMD registers (the kernel never uses them, so they hold the
/// state of the cell that last ran).
pub fn fp_save(state: &mut FpState) {
    fp_trap(false);
    // SAFETY: `state` is 16-byte aligned and at least 512 bytes; SSE is
    // enabled by `init_tags`.
    unsafe { asm!("fxsave64 [{}]", in(reg) state, options(nostack)) };
}

/// Loads the FP/SIMD registers for the cell about to run.
pub fn fp_load(state: &FpState) {
    fp_trap(false);
    // SAFETY: as for `fp_save`; `FXRSTOR` only loads valid images (reserved
    // MXCSR bits are cleared by the kernel's initial image and cannot be set
    // by a cell without faulting in `LDMXCSR` first).
    unsafe { asm!("fxrstor64 [{}]", in(reg) state, options(nostack)) };
}

/// Enables SSE for cells (and for the save and load above), SMEP and SMAP,
/// and PCIDs when the CPU has them; returns the highest tag cells may use
/// (4095), or 0 without PCIDs (every switch flushes). Call once, on the
/// boot CPU, before the others start.
pub fn init_tags() -> u16 {
    enable_sse();
    enable_user_guards();
    if core::arch::x86_64::__cpuid(1).ecx & (1 << 17) == 0 {
        return 0;
    }
    enable_pcid();
    PCID.store(true, Ordering::Relaxed);
    4095
}

/// Sets up a secondary CPU like [`init_tags`] did the boot CPU: SSE, SMEP
/// and SMAP, and PCIDs if the boot CPU uses them.
pub fn init_cpu() {
    enable_sse();
    enable_user_guards();
    if PCID.load(Ordering::Relaxed) {
        enable_pcid();
    }
}

fn enable_sse() {
    // SAFETY: CR0.MP set, CR0.EM clear, CR4.OSFXSR and OSXMMEXCPT set: SSE
    // works with FXSAVE support. The kernel is built without SSE.
    unsafe {
        asm!(
            "mov {t}, cr0",
            "and {t}, {em}",
            "or {t}, 2",
            "mov cr0, {t}",
            "mov {t}, cr4",
            "or {t}, {sse}",
            "mov cr4, {t}",
            t = out(reg) _,
            em = const !(1u64 << 2),
            sse = const (1u64 << 9) | (1u64 << 10),
            options(nostack)
        );
    }
}

/// CR4.SMEP (bit 20) and CR4.SMAP (bit 21), for the features CPUID lists.
const SMEP: u64 = 1 << 20;
const SMAP: u64 = 1 << 21;

/// The CR4 guards this CPU has: SMEP (`CPUID.7.0:EBX[7]`) and SMAP
/// (`EBX[20]`).
pub fn user_guards() -> u64 {
    if core::arch::x86_64::__cpuid(0).eax < 7 {
        return 0;
    }
    let ebx = core::arch::x86_64::__cpuid_count(7, 0).ebx;
    (if ebx & (1 << 7) != 0 { SMEP } else { 0 }) | (if ebx & (1 << 20) != 0 { SMAP } else { 0 })
}

/// Turns on SMEP (ring 0 never executes a user page) and SMAP (ring 0 never
/// reads or writes one) where the CPU has them (design D1 of
/// `add-user-access-guards`).
fn enable_user_guards() {
    let guards = user_guards();
    if guards == 0 {
        return;
    }
    // SAFETY: the kernel never executes, reads or writes a lower-half
    // address: cell memory is reached through the direct map, and the entry
    // code touches only per-CPU data, kernel stacks and the `Registers` in
    // the kernel heap. RFLAGS.AC, which would lift SMAP, is cleared on every
    // entry from a cell (SFMASK for SYSCALL, `popfq` for exceptions).
    unsafe {
        asm!(
            "mov {t}, cr4",
            "or {t}, {bits}",
            "mov cr4, {t}",
            t = out(reg) _,
            bits = in(reg) guards,
            options(nostack)
        );
    }
}

/// The guards turned on in CR4 on this CPU.
#[cfg(feature = "ktest")]
pub fn user_guards_on() -> u64 {
    let cr4: u64;
    // SAFETY: reading CR4 has no side effects.
    unsafe { asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags)) };
    cr4 & (SMEP | SMAP)
}

fn enable_pcid() {
    // SAFETY: CR4.PCIDE may be set while CR3 holds PCID 0, as it does (the
    // kernel's tables are loaded with no PCID bits).
    unsafe {
        asm!(
            "mov {t}, cr4",
            "or {t}, {bit}",
            "mov cr4, {t}",
            t = out(reg) _,
            bit = const 1u64 << 17,
            options(nostack)
        );
    }
}

/// RFLAGS direction flag (string instructions run backwards) and
/// alignment-check flag (also lifts SMAP in ring 0).
const DF: u64 = 1 << 10;
const AC: u64 = 1 << 18;

/// The kernel's own RFLAGS.
fn kernel_flags() -> u64 {
    let flags: u64;
    // SAFETY: `pushfq; pop` only reads RFLAGS (through the stack).
    unsafe { asm!("pushfq", "pop {}", out(reg) flags, options(preserves_flags)) };
    flags
}

fn load_cr3(value: u64) {
    // SAFETY: every value loaded is a cell root (sharing the kernel half) or
    // the kernel's own tables, so the kernel keeps running unchanged.
    unsafe { asm!("mov cr3, {}", in(reg) value, options(nostack, preserves_flags)) };
}

/// Runs the cell whose root table is `root`, tagged `tag` (PCID; 0 =
/// untagged), from `regs` until it makes a system call, is interrupted or
/// faults; `regs` then holds its state at that point.
///
/// `flush` drops the tag's translations on this CPU as the cell's tables
/// are loaded, so nothing a previous holder of the tag left (or that was
/// cached again while it was being released), and nothing stale from
/// before the cell's translations shrank on another CPU, can be used
/// (design D6 of `add-scheduler`, D5 of `add-smp-scheduling`).
pub fn enter_user(root: u64, tag: u16, flush: bool, regs: &mut Registers) -> Trap {
    let cr3 = root | tag as u64;
    let cpu = index();
    // One test of KPTI on the way (design D2 of `add-kpti`): each case has
    // its own copy of the entry code, and this path is the one from before.
    let vector = if KPTI.load(Ordering::Relaxed) {
        enter_with_kpti(cpu, cr3, tag, flush, regs)
    } else {
        if USER_CR3[cpu].swap(cr3, Ordering::Relaxed) != cr3 || flush {
            // `set_kpti` puts every CPU on the kernel's tables: the first
            // entry after it comes here and points `SYSCALL` at the entry
            // without KPTI.
            if KPTI_ENTRY[cpu].swap(false, Ordering::Relaxed) {
                super::cpu::set_syscall_entry(false);
            }
            let keep = PCID.load(Ordering::Relaxed) && tag != 0 && !flush;
            load_cr3(if keep { cr3 | NO_FLUSH } else { cr3 });
        }
        let sysret = prepare_entry(cpu, regs);
        // SAFETY: `regs` lives across the call; the entry code returns to
        // ring 3 with user selectors and only the user flags, and comes back
        // through the SYSCALL entry or a ring-3 exception, which restore the
        // kernel stack and callee-saved registers.
        unsafe { rhizo_enter_cell(regs, sysret as u64) }
    };
    debug_assert_eq!(
        kernel_flags() & (DF | AC),
        0,
        "the kernel runs with a cell's direction or alignment-check flag"
    );
    if vector == 256 {
        return Trap::Syscall;
    }
    let vector = vector as u8;
    if vector == 7 {
        return Trap::FpUnavailable;
    }
    if vector == super::irq::TIMER_VECTOR || vector == super::irq::WAKE_VECTOR {
        super::irq::acknowledge();
        return Trap::Interrupt;
    }
    if vector == super::irq::SAMPLE_VECTOR {
        super::pmu::rearm();
        super::irq::acknowledge();
        return Trap::Sample;
    }
    if vector == super::irq::SPURIOUS_VECTOR {
        return Trap::Interrupt;
    }
    Trap::Fault(UserFault(ExceptionInfo {
        vector,
        // Written by the trap path just before returning.
        error_code: super::cpu::trap_error(),
        rip: regs.pc,
        fault_address: (vector == exception::PAGE_FAULT).then(read_cr2),
    }))
}

/// [`enter_user`] with KPTI: the cell's CR3 is loaded by the entry code,
/// last, and the kernel's first on the way back. Kept apart so the path
/// without KPTI stays as it was.
#[cold]
#[inline(never)]
fn enter_with_kpti(cpu: usize, cr3: u64, tag: u16, flush: bool, regs: &mut Registers) -> u64 {
    if !KPTI_ENTRY[cpu].swap(true, Ordering::Relaxed) {
        super::cpu::set_syscall_entry(true);
    }
    // An untagged cell shares PCID 0 with the kernel: its CR3 always
    // flushes it, so no kernel translation stays cached while it runs.
    let keep = PCID.load(Ordering::Relaxed) && tag != 0 && !flush;
    super::cpu::PERCPU.0[cpu].set_user_cr3(if keep { cr3 | NO_FLUSH } else { cr3 });
    let sysret = prepare_entry(cpu, regs);
    // SAFETY: as in `enter_user`, through the entry pages and the cell's
    // tables, which map them.
    unsafe { rhizo_enter_cell_kpti(regs, sysret as u64) }
}

/// Loads the cell's thread pointer and says whether `SYSRETQ` can return
/// to it.
#[inline(always)]
fn prepare_entry(cpu: usize, regs: &Registers) -> bool {
    // SYSRETQ loads RIP from RCX and RFLAGS from R11, so it restores exactly
    // what IRETQ would when they already hold those values, as after a
    // system call. Threads of a cell share CR3, so this is a property of
    // the registers entered, not of the last exit (design D9 of
    // `add-threads`). It also needs a canonical lower-half RIP (else it
    // faults in ring 0).
    let user_flags = (regs.flags & 0xdd5) | 0x202;
    let sysret =
        regs.pc < 0x0000_8000_0000_0000 && regs.gpr[2] == regs.pc && regs.gpr[10] == user_flags;
    if LAST_FS[cpu].swap(regs.tls, Ordering::Relaxed) != regs.tls {
        // SAFETY: the FS base is the cell's thread pointer; the kernel does
        // not use FS.
        unsafe { wrmsr(FS_BASE, regs.tls) };
    }
    sysret
}

/// Switches CR3 back to the kernel's tables: call before freeing the tables
/// and the tag of the cell (`root`, `tag`).
pub fn release_user(_root: u64, _tag: u16) {
    // The tag's translations are dropped when its next cell first loads it
    // on each CPU (`enter_user` with `flush`): one CR3 load here, not two
    // (under nested virtualization each load can cost a VM exit).
    leave_user();
}

/// Switches this CPU's CR3 back to the kernel's tables, keeping the cell's
/// translations (another CPU may run it next, or free it; design D4 of
/// `add-smp-scheduling`).
pub fn leave_user() {
    if KPTI.load(Ordering::Relaxed) {
        return; // the kernel's tables are loaded already
    }
    let pcid = PCID.load(Ordering::Relaxed);
    let kernel = crate::memory::kernel_tables().root();
    USER_CR3[index()].store(kernel, Ordering::Relaxed);
    load_cr3(if pcid { kernel | NO_FLUSH } else { kernel });
}

/// Drops the translations of a cell that keeps running (its permissions
/// shrank, as after a fork); the next entry reloads its tables.
pub fn flush_user(root: u64, tag: u16) {
    let pcid = PCID.load(Ordering::Relaxed);
    // With KPTI the cell's tables cannot be loaded in the kernel (they map
    // no kernel code): the caller marks it stale everywhere
    // ([`stale_everywhere`]) and its next entry flushes its PCID.
    if pcid && tag != 0 && !KPTI.load(Ordering::Relaxed) {
        load_cr3(root | tag as u64); // without bit 63: flushes this PCID
    }
    release_user(root, tag);
}

/// Whether a TLB flush done in the kernel leaves every CPU, this one
/// included, with stale translations of the cell: yes with KPTI, where the
/// kernel runs on its own PCID and `INVLPG` cannot reach the cell's.
pub fn stale_everywhere() -> bool {
    KPTI.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// KPTI (design D1-D3 of `add-kpti`)
// ---------------------------------------------------------------------------

/// Whether cells' tables carry only the entry pages of the kernel half.
static KPTI: AtomicBool = AtomicBool::new(false);
/// The kernel half of cells' tables with KPTI ([`init_kpti`]).
static ENTRY_ROOT: AtomicU64 = AtomicU64::new(0);
const IA32_ARCH_CAPABILITIES: u32 = 0x10a;

/// Whether this CPU needs KPTI against Meltdown (rogue data cache load):
/// not on AMD or Hygon CPUs, which are not affected, nor on Intel CPUs
/// that say so (`IA32_ARCH_CAPABILITIES.RDCL_NO`); yes otherwise, as Linux
/// decides.
pub fn kpti_needed() -> bool {
    use core::arch::x86_64::{__cpuid, __cpuid_count};
    let vendor = __cpuid(0);
    let name = [vendor.ebx, vendor.edx, vendor.ecx];
    // "AuthenticAMD", "HygonGenuine"
    if name == [0x6874_7541, 0x6974_6e65, 0x444d_4163]
        || name == [0x6f67_7948, 0x6e65_476e, 0x656e_6975]
    {
        return false;
    }
    if vendor.eax >= 7 && __cpuid_count(7, 0).edx & (1 << 29) != 0 {
        // SAFETY: CPUID.7.0:EDX[29] says the MSR exists.
        let caps = unsafe { super::cpu::rdmsr(IA32_ARCH_CAPABILITIES) };
        return caps & 1 == 0;
    }
    true
}

/// Whether KPTI is on.
#[cfg_attr(
    not(any(feature = "ktest", feature = "bench-cell")),
    allow(dead_code, reason = "read by the tests and the benchmarks")
)]
pub fn kpti() -> bool {
    KPTI.load(Ordering::Relaxed)
}

/// Builds the kernel half cells' tables carry with KPTI: the entry code,
/// per-CPU data, GDTs, TSSs, IDT and trap stacks of the `cpus` CPUs online,
/// at their kernel addresses, supervisor only; nothing else of the kernel.
/// Call once, after the other CPUs started.
pub fn init_kpti(cpus: usize) {
    use rhizo_mm::paging::{PageFlags, PageSize, PageTable};
    let mut alloc = || crate::memory::with_frames(|f| f.alloc()).flatten();
    let pt = PageTable::<super::platform::Format>::new(&mut crate::memory::DirectMap, &mut alloc)
        .expect("a frame for the entry tables");
    let mut ranges: alloc::vec::Vec<_> = super::cpu::entry_ranges().into();
    for cpu in 1..cpus {
        let stacks = crate::smp::stacks(cpu);
        for top in [stacks.exception, stacks.double_fault] {
            ranges.push((top - 16 * 1024, top, false));
        }
    }
    for (start, end, code) in ranges {
        let flags = if code {
            PageFlags::empty()
        } else {
            PageFlags::WRITABLE | PageFlags::NO_EXECUTE
        };
        for virt in (start & !0xfff..end).step_by(4096) {
            let phys = crate::memory::translate(virt)
                .expect("entry page is mapped")
                .phys;
            pt.map(
                &mut crate::memory::DirectMap,
                &mut alloc,
                virt,
                phys & !0xfff,
                PageSize::Small,
                flags,
            )
            .expect("entry page mapped once");
        }
    }
    ENTRY_ROOT.store(pt.root(), Ordering::Relaxed);
}

/// Turns KPTI on or off for the cells created from now on: none may be
/// live. With it, every CPU's entry code loads the kernel's tables first.
pub fn set_kpti(on: bool) {
    leave_user();
    KPTI.store(on, Ordering::Relaxed);
    let root = crate::memory::kernel_tables().root();
    let kernel = match (on, PCID.load(Ordering::Relaxed)) {
        (false, _) => 0,
        (true, true) => root | NO_FLUSH,
        (true, false) => root,
    };
    for (percpu, user) in super::cpu::PERCPU.0.iter().zip(&USER_CR3) {
        percpu.set_kernel_cr3(kernel);
        percpu.set_user_cr3(0);
        user.store(root, Ordering::Relaxed); // every CPU is on the kernel's
    }
}

/// The kernel half a new cell's root carries: all of it, or with KPTI only
/// the entry pages.
pub fn cell_kernel_half() -> rhizo_mm::paging::PageTable<super::platform::Format> {
    match ENTRY_ROOT.load(Ordering::Relaxed) {
        root if KPTI.load(Ordering::Relaxed) && root != 0 => {
            rhizo_mm::paging::PageTable::from_root(root)
        }
        _ => crate::memory::kernel_tables(),
    }
}

/// Nothing to do on x86_64: instruction fetch sees data writes.
pub fn sync_code(_virt: u64, _len: u64) {}

/// Loads the tables rooted at `root` (sharing the kernel half), untagged:
/// for the self-checks that touch a user page from the kernel.
#[cfg(any(feature = "ktest-user-read", feature = "ktest-user-exec"))]
pub fn load_tables(root: u64) {
    USER_CR3[index()].store(root, Ordering::Relaxed);
    load_cr3(root);
}
