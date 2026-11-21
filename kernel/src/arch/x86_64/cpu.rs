//! CPU tables (GDT, TSS, IDT), exception entry and control registers.

use super::console::BootConsole;
use super::power::Machine;
use crate::sync::Global;
use core::arch::asm;
use core::sync::atomic::AtomicU64;
use rhizo_core::exception::{self, ExceptionInfo};
use rhizo_core::smp::MAX_CPUS;
use rhizo_hal::{ExitStatus, Power, Tlb};

const KERNEL_CODE: u16 = 0x08;
const TSS_SELECTOR: u16 = 0x18;
/// `STAR` base for `SYSRET`: user data = base + 8 (0x28), user code =
/// base + 16 (0x30), both with RPL 3.
const SYSRET_BASE: u64 = 0x20;
const DOUBLE_FAULT_IST: u8 = 1;
const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;
/// Stack the CPU switches to (TSS `RSP0`) on an exception from a cell.
const TRAP_STACK_SIZE: usize = 16 * 1024;

/// Page alignment around the entry code in builds that run cells, where
/// KPTI maps those pages for cells (design D2 of `add-kpti`); nothing in
/// the boot-only build, whose image stays as small as it was.
#[cfg(feature = "cells")]
macro_rules! entry_page {
    () => {
        ".balign 4096"
    };
}
#[cfg(not(feature = "cells"))]
macro_rules! entry_page {
    () => {
        ""
    };
}

// ---------------------------------------------------------------------------
// GDT and TSS
// ---------------------------------------------------------------------------

/// 64-bit Task State Segment (only the IST is used).
#[repr(C, packed(4))]
struct TaskStateSegment {
    reserved0: u32,
    rsp: [u64; 3],
    reserved1: u64,
    ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
}

/// Page-aligned, whole pages: with KPTI the cells' tables map it (the CPU
/// pushes there on an exception from a cell) and nothing else beside it.
#[cfg_attr(feature = "cells", repr(C, align(4096)))]
#[cfg_attr(not(feature = "cells"), repr(C, align(16)))]
struct Stack([u8; DOUBLE_FAULT_STACK_SIZE]);

/// The boot CPU's stacks (the other CPUs' are mapped by `smp`).
static DOUBLE_FAULT_STACK: Global<Stack> = Global::new(Stack([0; DOUBLE_FAULT_STACK_SIZE]));
static TRAP_STACK: Global<Stack> = Global::new(Stack([0; TRAP_STACK_SIZE]));

/// null, kernel code (0x08), kernel data (0x10), TSS (0x18, two slots,
/// filled per CPU), user data (0x28), user code (0x30): the order
/// `SYSCALL`/`SYSRET` need.
const GDT: [u64; 7] = [
    0,
    0x00af_9a00_0000_ffff,
    0x00cf_9200_0000_ffff,
    0,
    0,
    0x00cf_f200_0000_ffff,
    0x00af_fa00_0000_ffff,
];

/// A CPU's GDT and TSS: each CPU needs its own TSS (its stacks, and the
/// busy bit of its descriptor). Zero until the CPU loads them (so they
/// cost no image space).
struct Tables {
    gdt: [u64; 7],
    tss: TaskStateSegment,
}

/// Every CPU's GDT and TSS, alone on their pages: with KPTI the cells'
/// tables map them (the CPU reads them on an exception from a cell, and
/// `IRETQ` reads the GDT).
#[repr(C)]
#[cfg_attr(feature = "cells", repr(align(4096)))]
struct AllTables([Global<Tables>; MAX_CPUS]);

static TABLES: AllTables = AllTables(
    [const {
        Global::new(Tables {
            gdt: [0; 7],
            tss: TaskStateSegment {
                reserved0: 0,
                rsp: [0; 3],
                reserved1: 0,
                ist: [0; 7],
                reserved2: 0,
                reserved3: 0,
                iomap_base: 0,
            },
        })
    }; MAX_CPUS],
);

/// What each CPU keeps for itself, at its kernel GS base (the boot entry
/// code sets the boot CPU's; `smp` the others'). Cell code runs with its
/// own GS base: every entry from a cell and every return to one swap them
/// (`SWAPGS`), so a cell can never point the kernel's at its memory.
#[repr(C)]
#[derive(Debug)]
pub struct PerCpu {
    /// The CPU's number: 0 for the boot CPU (spec `multi-core`).
    index: AtomicU64,
    /// Kernel stack pointer saved while a cell runs on this CPU.
    kernel_rsp: AtomicU64,
    /// The `Registers` of the cell running on this CPU.
    cell_regs: AtomicU64,
    /// The cell's stack pointer at `SYSCALL`.
    user_rsp: AtomicU64,
    /// Error code of the last exception from a cell.
    trap_error: AtomicU64,
    /// With KPTI, the CR3 the entry code loads first (the kernel's tables);
    /// 0 without (design D2 of `add-kpti`).
    kernel_cr3: AtomicU64,
    /// With KPTI, the CR3 loaded last before returning to the cell (its own
    /// tables); 0 without.
    user_cr3: AtomicU64,
    /// The cell's RDI while the return path needs a register for CR3.
    scratch: AtomicU64,
    /// Top of this CPU's trap stack (TSS `RSP0`), where `IRETQ` to a cell
    /// finds its frame (mapped for cells with KPTI).
    trap_top: AtomicU64,
}

/// Every CPU's per-CPU data, alone on its pages: with KPTI the cells'
/// tables map them (the entry code reads them before switching tables).
#[repr(C)]
#[cfg_attr(feature = "cells", repr(align(4096)))]
pub(super) struct AllPerCpu(pub(super) [PerCpu; MAX_CPUS]);

/// Per-CPU data of every CPU, by number.
#[unsafe(export_name = "rhizo_percpu")]
pub(super) static PERCPU: AllPerCpu = AllPerCpu(
    [const {
        PerCpu {
            index: AtomicU64::new(0),
            kernel_rsp: AtomicU64::new(0),
            cell_regs: AtomicU64::new(0),
            user_rsp: AtomicU64::new(0),
            trap_error: AtomicU64::new(0),
            kernel_cr3: AtomicU64::new(0),
            user_cr3: AtomicU64::new(0),
            scratch: AtomicU64::new(0),
            trap_top: AtomicU64::new(0),
        }
    }; MAX_CPUS],
);

impl PerCpu {
    /// Sets the CR3 the entry code loads on entry from a cell (0: none).
    #[cfg(feature = "cells")]
    pub(super) fn set_kernel_cr3(&self, cr3: u64) {
        self.kernel_cr3
            .store(cr3, core::sync::atomic::Ordering::Relaxed);
    }

    /// Sets the CR3 loaded just before returning to a cell (0: none).
    #[cfg(feature = "cells")]
    pub(super) fn set_user_cr3(&self, cr3: u64) {
        self.user_cr3
            .store(cr3, core::sync::atomic::Ordering::Relaxed);
    }
}

/// The number of the CPU running this code.
pub fn index() -> usize {
    let index: u64;
    // SAFETY: GS points at this CPU's `PerCpu` whenever kernel code runs
    // (set before any Rust code, swapped only around cell code).
    unsafe { asm!("mov {}, gs:[0]", out(reg) index, options(nostack, readonly, preserves_flags)) };
    index as usize
}

/// Error code of the last exception from a cell on this CPU.
#[cfg(feature = "cells")]
pub(super) fn trap_error() -> u64 {
    PERCPU.0[index()]
        .trap_error
        .load(core::sync::atomic::Ordering::Relaxed)
}

/// Prepares the per-CPU data of secondary CPU `cpu`; returns its address
/// (for its GS base).
#[cfg(feature = "cells")]
pub(super) fn percpu(cpu: usize) -> u64 {
    PERCPU.0[cpu]
        .index
        .store(cpu as u64, core::sync::atomic::Ordering::Relaxed);
    &PERCPU.0[cpu] as *const PerCpu as u64
}

#[repr(C, packed)]
struct DescriptorPointer {
    limit: u16,
    base: u64,
}

fn tss_descriptor(base: u64) -> [u64; 2] {
    let limit = (size_of::<TaskStateSegment>() - 1) as u64;
    let low = (limit & 0xffff)
        | ((base & 0xff_ffff) << 16)
        | (0x89 << 40) // present, 64-bit available TSS
        | (((limit >> 16) & 0xf) << 48)
        | (((base >> 24) & 0xff) << 56);
    [low, base >> 32]
}

/// Loads this CPU's GDT and TSS, with its double-fault and trap stacks.
fn load_gdt_and_tss(stack_top: u64, trap_top: u64) {
    PERCPU.0[index()]
        .trap_top
        .store(trap_top, core::sync::atomic::Ordering::Relaxed);
    let pointer = TABLES.0[index()].with(|t| {
        t.gdt = GDT;
        t.tss.iomap_base = size_of::<TaskStateSegment>() as u16;
        t.tss.ist[(DOUBLE_FAULT_IST - 1) as usize] = stack_top;
        t.tss.rsp[0] = trap_top;
        let [low, high] = tss_descriptor(&t.tss as *const TaskStateSegment as u64);
        t.gdt[3] = low;
        t.gdt[4] = high;
        DescriptorPointer {
            limit: (size_of::<[u64; 7]>() - 1) as u16,
            base: t.gdt.as_ptr() as u64,
        }
    });
    // SAFETY: the GDT is static and well-formed (same code/data selectors as
    // the boot GDT), so reloading CS through a far return and the data
    // segments keeps executing the same code; the TSS descriptor points to a
    // static TSS.
    unsafe {
        asm!(
            "lgdt [{ptr}]",
            "push {code}",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            "mov {tmp:x}, 0x10",
            "mov ss, {tmp:x}",
            // Null data segments: valid in 64-bit mode at any privilege, so
            // returning to a cell never reloads them.
            "xor {tmp:e}, {tmp:e}",
            "mov ds, {tmp:x}",
            "mov es, {tmp:x}",
            "ltr {tss:x}",
            ptr = in(reg) &pointer,
            code = const KERNEL_CODE as u64,
            tmp = out(reg) _,
            tss = in(reg) TSS_SELECTOR,
        );
    }
}

// ---------------------------------------------------------------------------
// IDT and exception entry
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
#[repr(C)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    attributes: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const MISSING: Self = Self {
        offset_low: 0,
        selector: 0,
        ist: 0,
        attributes: 0,
        offset_mid: 0,
        offset_high: 0,
        reserved: 0,
    };

    fn interrupt_gate(handler: u64, ist: u8) -> Self {
        Self {
            offset_low: handler as u16,
            selector: KERNEL_CODE,
            ist,
            attributes: 0x8E, // present, DPL 0, 64-bit interrupt gate
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            reserved: 0,
        }
    }
}

/// The IDT, alone on its pages (with KPTI the cells' tables map it).
#[repr(C)]
#[cfg_attr(feature = "cells", repr(align(4096)))]
struct Idt(Global<[IdtEntry; 256]>);

static IDT: Idt = Idt(Global::new([IdtEntry::MISSING; 256]));

// Exception stubs: each pushes the error code (or 0) and the vector, then
// jumps to a common path that calls `rhizo_exception_handler` with a pointer
// to `ExceptionFrame`.
core::arch::global_asm!(
    r#"
    .macro RHIZO_ISR_NOERR n
rhizo_isr_\n:
    pushq $0
    pushq $\n
    jmp rhizo_isr_common
    .endm

    .macro RHIZO_ISR_ERR n
rhizo_isr_\n:
    pushq $\n
    jmp rhizo_isr_common
    .endm

    // With KPTI, cells' tables map these pages (design D2 of `add-kpti`):
    // the code that runs before the switch to the kernel's tables, alone
    // on whole pages.
    .section .text.rhizo_entry_a,"ax",@progbits
    "#,
    entry_page!(),
    r#"
    .global rhizo_entry_a_start
rhizo_entry_a_start:
    .irp n, 0,1,2,3,4,5,6,7,9,15,16,18,19,20,22,23,24,25,26,27,28,31
    RHIZO_ISR_NOERR \n
    .endr
    .irp n, 8,10,11,12,13,14,17,21,29,30
    RHIZO_ISR_ERR \n
    .endr
    RHIZO_ISR_NOERR 32
    RHIZO_ISR_NOERR 33
    RHIZO_ISR_NOERR 34
    RHIZO_ISR_NOERR 255

rhizo_isr_common:
    testb $3, 24(%rsp)              // CS of the interrupted code: from a cell?
    jnz rhizo_user_trap
    cmpq $32, (%rsp)                // a device interrupt in ring 0 can only
    jae 3f                          // come from `irq::wait`: just return
    movq %rsp, %rdi
    andq $-16, %rsp
    call rhizo_exception_handler
    ud2
3:  addq $16, %rsp
    iretq
    "#,
    entry_page!(),
    r#"
    .global rhizo_entry_a_end
rhizo_entry_a_end:

    .section .rodata
    .balign 8
    .global rhizo_isr_table
rhizo_isr_table:
    .irp n, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    .quad rhizo_isr_\n
    .endr
    .global rhizo_isr_device
rhizo_isr_device:
    .quad rhizo_isr_32
    .quad rhizo_isr_33
    .quad rhizo_isr_34
    .quad rhizo_isr_255

    .text
    "#,
    options(att_syntax)
);

unsafe extern "C" {
    static rhizo_isr_table: [u64; 32];
    static rhizo_isr_device: [u64; 4];
}

/// Stack layout built by the stubs and the CPU.
#[repr(C)]
struct ExceptionFrame {
    vector: u64,
    error_code: u64,
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

#[unsafe(no_mangle)]
extern "C" fn rhizo_exception_handler(frame: &ExceptionFrame) -> ! {
    let vector = frame.vector as u8;
    let info = ExceptionInfo {
        vector,
        error_code: frame.error_code,
        rip: frame.rip,
        fault_address: (vector == exception::PAGE_FAULT).then(read_cr2),
    };
    exception::report_exception(&mut BootConsole, &info);
    Machine.exit(ExitStatus::Failure)
}

fn load_idt() {
    // SAFETY: `rhizo_isr_table` is a read-only table of 32 stub addresses
    // emitted by the assembly above.
    let stubs = unsafe { &*core::ptr::addr_of!(rhizo_isr_table) };
    let pointer = IDT.0.with(|idt| {
        for (vector, &stub) in stubs.iter().enumerate() {
            let ist = if vector == exception::DOUBLE_FAULT as usize {
                DOUBLE_FAULT_IST
            } else {
                0
            };
            idt[vector] = IdtEntry::interrupt_gate(stub, ist);
        }
        // SAFETY: as above, four more stubs (timer, wake-up, sampling and spurious
        // vectors).
        let [timer, wake, sample, spurious] = unsafe { *core::ptr::addr_of!(rhizo_isr_device) };
        idt[32] = IdtEntry::interrupt_gate(timer, 0);
        idt[33] = IdtEntry::interrupt_gate(wake, 0);
        idt[34] = IdtEntry::interrupt_gate(sample, 0);
        idt[255] = IdtEntry::interrupt_gate(spurious, 0);
        DescriptorPointer {
            limit: (size_of::<[IdtEntry; 256]>() - 1) as u16,
            base: idt.as_ptr() as u64,
        }
    });
    // SAFETY: the IDT is static and every present entry points to a valid
    // stub in kernel code.
    unsafe { asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack)) };
}

/// Installs the boot CPU's GDT, TSS and IDT and enables `SYSCALL`. Call
/// once, early in boot.
pub fn init() {
    let stack_top = DOUBLE_FAULT_STACK.with(|s| s.0.as_ptr_range().end as u64);
    let trap_top = TRAP_STACK.with(|s| s.0.as_ptr_range().end as u64);
    init_cpu(stack_top, trap_top);
}

/// Installs this CPU's GDT and TSS (with its double-fault and trap
/// stacks), the IDT, and enables `SYSCALL`.
/// What cells' tables map of the kernel with KPTI, besides the other CPUs'
/// stacks: `(start, end, code)` virtual ranges, each on whole pages of its
/// own (design D2 of `add-kpti`).
#[cfg(feature = "cells")]
pub(super) fn entry_ranges() -> [(u64, u64, bool); 7] {
    unsafe extern "C" {
        static rhizo_entry_a_start: u8;
        static rhizo_entry_a_end: u8;
        static rhizo_entry_b_start: u8;
        static rhizo_entry_b_end: u8;
    }
    fn span<T>(value: &T) -> (u64, u64, bool) {
        let start = value as *const T as u64;
        (start, start + size_of::<T>() as u64, false)
    }
    // Only the addresses of the symbols are taken: no `unsafe` needed.
    let code = [
        (
            core::ptr::addr_of!(rhizo_entry_a_start) as u64,
            core::ptr::addr_of!(rhizo_entry_a_end) as u64,
            true,
        ),
        (
            core::ptr::addr_of!(rhizo_entry_b_start) as u64,
            core::ptr::addr_of!(rhizo_entry_b_end) as u64,
            true,
        ),
    ];
    let stack = |s: &Global<Stack>| {
        let r = s.with(|s| s.0.as_ptr_range());
        (r.start as u64, r.end as u64, false)
    };
    [
        code[0],
        code[1],
        span(&PERCPU),
        span(&TABLES),
        span(&IDT),
        stack(&TRAP_STACK),
        stack(&DOUBLE_FAULT_STACK),
    ]
}

pub(super) fn init_cpu(double_fault_stack: u64, trap_stack: u64) {
    load_gdt_and_tss(double_fault_stack, trap_stack);
    load_idt();
    // SAFETY: enabling SYSCALL and pointing it at the entry stub below; the
    // selectors match the GDT, and SFMASK clears IF, DF, TF and AC on entry.
    unsafe {
        wrmsr(EFER, rdmsr(EFER) | 1);
        wrmsr(STAR, (SYSRET_BASE << 48) | ((KERNEL_CODE as u64) << 32));
        wrmsr(LSTAR, rhizo_syscall_entry as *const () as u64);
        wrmsr(SFMASK, 0x4_7700);
    }
}

const EFER: u32 = 0xC000_0080;
const STAR: u32 = 0xC000_0081;
const LSTAR: u32 = 0xC000_0082;
const SFMASK: u32 = 0xC000_0084;

/// # Safety
/// `msr` must exist on this CPU.
pub(super) unsafe fn rdmsr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    // SAFETY: guaranteed by the caller.
    unsafe {
        asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high, options(nomem, nostack))
    };
    ((high as u64) << 32) | low as u64
}

/// # Safety
/// Writing `value` to `msr` must be valid and keep the kernel running.
pub(super) unsafe fn wrmsr(msr: u32, value: u64) {
    // SAFETY: guaranteed by the caller.
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") value as u32, in("edx") (value >> 32) as u32, options(nostack))
    };
}

// ---------------------------------------------------------------------------
// Cells: ring 3 entry and exit (design decision D4)
// ---------------------------------------------------------------------------
//
// `rhizo_enter_cell(regs, sysret)` saves the kernel's callee-saved registers
// and stack pointer, loads the cell's registers and drops to ring 3 with
// `SYSRETQ` (after a system call, when RCX/R11 hold nothing the cell needs)
// or `IRETQ` (any other time). The cell comes back through `SYSCALL`
// (returns 256) or an exception from ring 3 (returns the vector): both save
// its registers into `regs` and resume the kernel where it called in.
// What the paths share lives in this CPU's `PerCpu` (GS: index 0, kernel
// RSP 8, cell registers 16, user RSP 24, trap error 32, kernel CR3 40,
// user CR3 48, scratch 56, trap stack top 64), and `SWAPGS` brackets cell
// code. With KPTI (design D2 of `add-kpti`) a cell's tables map only these
// pages of code, the per-CPU data, GDT, TSS, IDT and trap stacks: entries
// load the kernel's CR3 first, and returns load the cell's last, through
// a register stashed in per-CPU data. Exceptions share one entry, where a
// zero CR3 means no switch; entering a cell and system calls have a copy
// for each case.
//
// `Registers` layout: pc 0, sp 8, gpr 16 + 8n (rax rbx rcx rdx rsi rdi rbp
// r8-r15), tls 264, flags 272.
core::arch::global_asm!(
    r#"
    .section .text.rhizo_entry_b,"ax",@progbits
    "#,
    entry_page!(),
    r#"
    // Entering a cell and its system calls come in two copies (design D5
    // of `add-kpti`): with KPTI, in the entry pages, switching CR3; without,
    // in the kernel's code, with nothing of KPTI on the way.
    .macro RHIZO_ENTER_CELL kpti
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15
    mov gs:[8], rsp
    mov gs:[16], rdi
    .if \kpti
    mov rax, [rdi + 56]             // the cell's RDI, loaded last
    mov gs:[56], rax
    .endif
    mov rax, [rdi + 272]
    and eax, 0xdd5                  // user flags only: CF PF AF ZF SF DF OF
    or eax, 0x202                   // reserved bit, IF: preemptible
    test rsi, rsi
    jz 2f
    mov r11, rax
    mov rcx, [rdi + 0]
    mov rsp, [rdi + 8]
    mov rax, [rdi + 16]
    mov rbx, [rdi + 24]
    mov rdx, [rdi + 40]
    mov rsi, [rdi + 48]
    mov rbp, [rdi + 64]
    mov r8, [rdi + 72]
    mov r9, [rdi + 80]
    mov r10, [rdi + 88]
    mov r12, [rdi + 104]
    mov r13, [rdi + 112]
    mov r14, [rdi + 120]
    mov r15, [rdi + 128]
    .if \kpti
    mov rdi, gs:[48]                // the cell's CR3, last
    mov cr3, rdi
    mov rdi, gs:[56]
    .else
    mov rdi, [rdi + 56]
    .endif
    swapgs
    sysretq
2:
    .if \kpti
    mov rsp, gs:[64]                // the trap stack, mapped for the cell
    .endif
    push 0x2b                       // user SS
    push qword ptr [rdi + 8]
    push rax
    push 0x33                       // user CS
    push qword ptr [rdi + 0]
    mov rax, [rdi + 16]
    mov rbx, [rdi + 24]
    mov rcx, [rdi + 32]
    mov rdx, [rdi + 40]
    mov rsi, [rdi + 48]
    mov rbp, [rdi + 64]
    mov r8, [rdi + 72]
    mov r9, [rdi + 80]
    mov r10, [rdi + 88]
    mov r11, [rdi + 96]
    mov r12, [rdi + 104]
    mov r13, [rdi + 112]
    mov r14, [rdi + 120]
    mov r15, [rdi + 128]
    .if \kpti
    mov rdi, gs:[48]                // the cell's CR3, last
    mov cr3, rdi
    mov rdi, gs:[56]
    .else
    mov rdi, [rdi + 56]
    .endif
    swapgs
    iretq
    .endm

    // SYSCALL: RIP in RCX, RFLAGS in R11, still on the cell's stack.
    .macro RHIZO_SYSCALL_ENTRY kpti
    swapgs
    mov gs:[24], rsp
    .if \kpti
    mov rsp, gs:[40]                // the kernel's CR3, first
    mov cr3, rsp
    .endif
    mov rsp, gs:[16]
    mov [rsp + 16], rax
    mov [rsp + 24], rbx
    mov [rsp + 32], rcx
    mov [rsp + 40], rdx
    mov [rsp + 48], rsi
    mov [rsp + 56], rdi
    mov [rsp + 64], rbp
    mov [rsp + 72], r8
    mov [rsp + 80], r9
    mov [rsp + 88], r10
    mov [rsp + 96], r11
    mov [rsp + 104], r12
    mov [rsp + 112], r13
    mov [rsp + 120], r14
    mov [rsp + 128], r15
    mov [rsp + 0], rcx
    mov [rsp + 272], r11
    mov rax, gs:[24]
    mov [rsp + 8], rax
    mov eax, 256
    jmp rhizo_back_to_kernel
    .endm

    .global rhizo_entry_b_start
rhizo_entry_b_start:
    .global rhizo_enter_cell_kpti
rhizo_enter_cell_kpti:
    RHIZO_ENTER_CELL 1
    .global rhizo_syscall_entry_kpti
rhizo_syscall_entry_kpti:
    RHIZO_SYSCALL_ENTRY 1

    // Exception from ring 3, on the TSS RSP0 stack: vector, error code,
    // RIP, CS, RFLAGS, RSP, SS (pushed by the stub and the CPU).
rhizo_user_trap:
    swapgs
    push 2                          // RFLAGS: only the reserved bit. The
    popfq                           // cell's DF and AC (set by POPF in ring
                                    // 3; an interrupt gate keeps them) must
                                    // not reach Rust code or lift SMAP
    push rax
    mov rax, gs:[40]                // KPTI: the kernel's CR3
    test rax, rax
    jz 1f
    mov cr3, rax
1:  mov rax, gs:[16]
    mov [rax + 24], rbx
    mov [rax + 32], rcx
    mov [rax + 40], rdx
    mov [rax + 48], rsi
    mov [rax + 56], rdi
    mov [rax + 64], rbp
    mov [rax + 72], r8
    mov [rax + 80], r9
    mov [rax + 88], r10
    mov [rax + 96], r11
    mov [rax + 104], r12
    mov [rax + 112], r13
    mov [rax + 120], r14
    mov [rax + 128], r15
    mov rbx, [rsp]
    mov [rax + 16], rbx
    mov rbx, [rsp + 24]
    mov [rax + 0], rbx
    mov rbx, [rsp + 48]
    mov [rax + 8], rbx
    mov rbx, [rsp + 40]
    mov [rax + 272], rbx
    mov rbx, [rsp + 16]
    mov gs:[32], rbx
    mov rax, [rsp + 8]
rhizo_back_to_kernel:
    mov rsp, gs:[8]
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx
    ret
    "#,
    entry_page!(),
    r#"
    .global rhizo_entry_b_end
rhizo_entry_b_end:
    .text
    .global rhizo_enter_cell
rhizo_enter_cell:
    RHIZO_ENTER_CELL 0
    .global rhizo_syscall_entry
rhizo_syscall_entry:
    RHIZO_SYSCALL_ENTRY 0
    "#
);

unsafe extern "C" {
    fn rhizo_syscall_entry();
    #[cfg(feature = "cells")]
    fn rhizo_syscall_entry_kpti();
}

/// Points this CPU's `SYSCALL` at the entry with KPTI or the one without.
#[cfg(feature = "cells")]
pub(super) fn set_syscall_entry(kpti: bool) {
    let entry = if kpti {
        rhizo_syscall_entry_kpti as *const ()
    } else {
        rhizo_syscall_entry as *const ()
    };
    // SAFETY: both are complete system-call entries; which one runs only
    // decides whether CR3 is switched, as the per-CPU KPTI state expects.
    unsafe { wrmsr(LSTAR, entry as u64) };
}

// ---------------------------------------------------------------------------
// Control registers and TLB
// ---------------------------------------------------------------------------

/// Whether the CPU supports 1 GiB pages (`CPUID.80000001h:EDX[26]`).
pub fn supports_1gib_pages() -> bool {
    // CPUID leaf 0x8000_0001 exists on every x86_64 CPU.
    let leaf = core::arch::x86_64::__cpuid(0x8000_0001);
    leaf.edx & (1 << 26) != 0
}

/// Executes an undefined instruction (self-check `ktest-invalid-opcode`).
#[cfg(feature = "ktest-invalid-opcode")]
pub fn execute_invalid_instruction() {
    // SAFETY: deliberately invalid; the exception ends the run.
    unsafe { asm!("ud2") };
}

/// Faulting address of the last page fault.
pub fn read_cr2() -> u64 {
    let value: u64;
    // SAFETY: reading CR2 has no side effects.
    unsafe { asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

/// Switches to the page tables rooted at physical address `root`.
///
/// # Safety
/// The new tables must map the currently executing code, the stack and all
/// kernel data at the same virtual addresses.
pub unsafe fn activate(root: u64, _alloc: &mut impl FnMut() -> Option<u64>) {
    // SAFETY: guaranteed by the caller.
    unsafe { asm!("mov cr3, {}", in(reg) root, options(nostack, preserves_flags)) };
}

/// The CPU's TLB, flushed one page at a time with `invlpg`.
#[derive(Debug)]
pub struct CpuTlb;

impl Tlb for CpuTlb {
    fn flush_page(&mut self, virt: u64) {
        // SAFETY: `invlpg` only drops a cached translation.
        unsafe { asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags)) };
    }
}

/// Drops every translation and every paging-structure cache entry of this
/// CPU, for every PCID, global ones included: a write to CR4 that changes
/// CR4.PGE does, so it is toggled and set back.
#[cfg(feature = "cells")]
pub fn flush_all() {
    const PGE: u64 = 1 << 7;
    // SAFETY: only CR4.PGE changes, and back; both writes only flush.
    unsafe {
        asm!(
            "mov {t}, cr4",
            "xor {t}, {pge}",
            "mov cr4, {t}",
            "xor {t}, {pge}",
            "mov cr4, {t}",
            t = out(reg) _,
            pge = in(reg) PGE,
            options(nostack, preserves_flags)
        );
    }
}
