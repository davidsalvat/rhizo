//! CPU tables (GDT, TSS, IDT), exception entry and control registers.

use super::console::BootConsole;
use super::power::Machine;
use crate::sync::Global;
use core::arch::asm;
use rhizo_core::exception::{self, ExceptionInfo};
use rhizo_hal::{ExitStatus, Power, Tlb};

const KERNEL_CODE: u16 = 0x08;
const TSS_SELECTOR: u16 = 0x18;
const DOUBLE_FAULT_IST: u8 = 1;
const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;

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

#[repr(C, align(16))]
struct Stack([u8; DOUBLE_FAULT_STACK_SIZE]);

static DOUBLE_FAULT_STACK: Global<Stack> = Global::new(Stack([0; DOUBLE_FAULT_STACK_SIZE]));

static TSS: Global<TaskStateSegment> = Global::new(TaskStateSegment {
    reserved0: 0,
    rsp: [0; 3],
    reserved1: 0,
    ist: [0; 7],
    reserved2: 0,
    reserved3: 0,
    iomap_base: size_of::<TaskStateSegment>() as u16,
});

/// null, kernel code (0x08), kernel data (0x10), TSS (0x18, two slots).
static GDT: Global<[u64; 5]> = Global::new([0, 0x00af_9a00_0000_ffff, 0x00cf_9200_0000_ffff, 0, 0]);

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

fn load_gdt_and_tss() {
    let stack_top = DOUBLE_FAULT_STACK.with(|s| s.0.as_ptr_range().end as u64);
    let tss_base = TSS.with(|tss| {
        tss.ist[(DOUBLE_FAULT_IST - 1) as usize] = stack_top;
        tss as *const TaskStateSegment as u64
    });
    let pointer = GDT.with(|gdt| {
        let [low, high] = tss_descriptor(tss_base);
        gdt[3] = low;
        gdt[4] = high;
        DescriptorPointer {
            limit: (size_of::<[u64; 5]>() - 1) as u16,
            base: gdt.as_ptr() as u64,
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
            "mov ds, {tmp:x}",
            "mov es, {tmp:x}",
            "mov ss, {tmp:x}",
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

static IDT: Global<[IdtEntry; 256]> = Global::new([IdtEntry::MISSING; 256]);

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

    .text
    .irp n, 0,1,2,3,4,5,6,7,9,15,16,18,19,20,22,23,24,25,26,27,28,31
    RHIZO_ISR_NOERR \n
    .endr
    .irp n, 8,10,11,12,13,14,17,21,29,30
    RHIZO_ISR_ERR \n
    .endr

rhizo_isr_common:
    movq %rsp, %rdi
    andq $-16, %rsp
    call rhizo_exception_handler
    ud2

    .section .rodata
    .balign 8
    .global rhizo_isr_table
rhizo_isr_table:
    .irp n, 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    .quad rhizo_isr_\n
    .endr

    .text
    "#,
    options(att_syntax)
);

unsafe extern "C" {
    static rhizo_isr_table: [u64; 32];
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
    let pointer = IDT.with(|idt| {
        for (vector, &stub) in stubs.iter().enumerate() {
            let ist = if vector == exception::DOUBLE_FAULT as usize {
                DOUBLE_FAULT_IST
            } else {
                0
            };
            idt[vector] = IdtEntry::interrupt_gate(stub, ist);
        }
        DescriptorPointer {
            limit: (size_of::<[IdtEntry; 256]>() - 1) as u16,
            base: idt.as_ptr() as u64,
        }
    });
    // SAFETY: the IDT is static and every present entry points to a valid
    // stub in kernel code.
    unsafe { asm!("lidt [{}]", in(reg) &pointer, options(readonly, nostack)) };
}

/// Installs the GDT, TSS and IDT. Call once, early in boot.
pub fn init() {
    load_gdt_and_tss();
    load_idt();
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

#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests; cells use it from H2")
)]
/// The CPU's TLB, flushed one page at a time with `invlpg`.
#[derive(Debug)]
pub struct CpuTlb;

impl Tlb for CpuTlb {
    fn flush_page(&mut self, virt: u64) {
        // SAFETY: `invlpg` only drops a cached translation.
        unsafe { asm!("invlpg [{}]", in(reg) virt, options(nostack, preserves_flags)) };
    }
}
