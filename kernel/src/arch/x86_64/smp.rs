//! Starting the other CPUs (spec `multi-core`): the CPUs come from the
//! ACPI MADT, each is started with INIT and STARTUP IPIs into a real-mode
//! trampoline copied below 1 MiB, which goes through protected mode to long
//! mode on a temporary root table (the kernel half plus the trampoline page
//! mapped where it is) and calls `rhizo_ap_main` on the CPU's own stacks.

use super::cpu::{self, percpu};
use super::irq;
use crate::memory::{self, DirectMap};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use rhizo_core::acpi;
use rhizo_core::smp::{Cpus, MAX_CPUS};
use rhizo_hal::{CycleCounter, PhysicalMemory};
use rhizo_mm::FRAME_SIZE;
use rhizo_mm::memmap::{BootInfo, RegionKind};
use rhizo_mm::paging::{PageFlags, PageSize, PageTable};

/// Physical page the trampoline is copied to: the STARTUP IPI's vector is
/// its page number. Below 1 MiB, which the kernel never hands out, and
/// clear of the boot information on every supported VMM.
const TRAMPOLINE: u64 = 0x8000;
/// How long a started CPU may take to reach the kernel.
const ARRIVAL_NS: u64 = 2_000_000_000;
/// Wait before repeating the STARTUP IPI (the MP specification's second
/// one, for a CPU that missed the first).
const RETRY_NS: u64 = 10_000_000;

// The trampoline, assembled into `.rodata` and copied to `TRAMPOLINE`. It
// runs at any page below 1 MiB: real-mode addresses are offsets from CS,
// later ones from its physical base in EBX. The kernel fills the words at
// the end: the far-jump targets and GDT base (physical), the root table,
// the CPU's stack, per-CPU data, number, and `rhizo_ap_main`.
core::arch::global_asm!(
    r#"
    .section .rodata.ap_trampoline, "a"
    .balign 16
    .global rhizo_ap_trampoline
rhizo_ap_trampoline:
    .code16
    cli
    cld
    movw %cs, %ax
    movw %ax, %ds
    movzwl %ax, %ebx
    shll $4, %ebx
    lgdtl (rhizo_ap_gdt_ptr - rhizo_ap_trampoline)
    movl %cr0, %eax
    orl $1, %eax
    movl %eax, %cr0
    ljmpl *(rhizo_ap_pm_target - rhizo_ap_trampoline)

    .code32
    .global rhizo_ap_pm
rhizo_ap_pm:
    movw $0x10, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %ss
    movl %cr4, %eax
    orl $(1 << 5), %eax                 /* PAE */
    movl %eax, %cr4
    movl (rhizo_ap_cr3 - rhizo_ap_trampoline)(%ebx), %eax
    movl %eax, %cr3
    movl $0xC0000080, %ecx              /* EFER: SCE, LME, NXE */
    rdmsr
    orl $((1 << 0) | (1 << 8) | (1 << 11)), %eax
    wrmsr
    movl %cr0, %eax                     /* PG, WP; caches on (CD, NW */
    orl $0x80010001, %eax               /* are set after INIT) */
    andl $0x9fffffff, %eax
    movl %eax, %cr0
    ljmpl *(rhizo_ap_lm_target - rhizo_ap_trampoline)(%ebx)

    .code64
    .global rhizo_ap_lm
rhizo_ap_lm:
    movl %ebx, %ebx
    movq (rhizo_ap_stack - rhizo_ap_trampoline)(%rbx), %rsp
    movq (rhizo_ap_percpu - rhizo_ap_trampoline)(%rbx), %rax
    movq %rax, %rdx
    shrq $32, %rdx
    movl $0xC0000101, %ecx              /* GS base: this CPU's data */
    wrmsr
    movq (rhizo_ap_index - rhizo_ap_trampoline)(%rbx), %rdi
    movq (rhizo_ap_entry - rhizo_ap_trampoline)(%rbx), %rax
    xorl %ebp, %ebp
    call *%rax
1:  cli
    hlt
    jmp 1b

    /* Accessed bits preset: loading a selector would otherwise write
       the descriptor, and the page is read-only once paging is on. */
    .balign 8
    .global rhizo_ap_gdt
rhizo_ap_gdt:
    .quad 0
    .quad 0x00cf9b000000ffff            /* 0x08: 32-bit code */
    .quad 0x00cf93000000ffff            /* 0x10: data */
    .quad 0x00af9b000000ffff            /* 0x18: 64-bit code */
    .global rhizo_ap_gdt_ptr
rhizo_ap_gdt_ptr:
    .word 31
    .long 0
    .balign 4
    .global rhizo_ap_pm_target
rhizo_ap_pm_target:
    .long 0
    .word 0x08
    .balign 4
    .global rhizo_ap_lm_target
rhizo_ap_lm_target:
    .long 0
    .word 0x18
    .balign 8
    .global rhizo_ap_cr3
rhizo_ap_cr3: .quad 0
    .global rhizo_ap_stack
rhizo_ap_stack: .quad 0
    .global rhizo_ap_percpu
rhizo_ap_percpu: .quad 0
    .global rhizo_ap_index
rhizo_ap_index: .quad 0
    .global rhizo_ap_entry
rhizo_ap_entry: .quad 0
    .global rhizo_ap_trampoline_end
rhizo_ap_trampoline_end:
    .text
    "#,
    options(att_syntax)
);

unsafe extern "C" {
    static rhizo_ap_trampoline: u8;
    static rhizo_ap_pm: u8;
    static rhizo_ap_lm: u8;
    static rhizo_ap_gdt: u8;
    static rhizo_ap_gdt_ptr: u8;
    static rhizo_ap_pm_target: u8;
    static rhizo_ap_lm_target: u8;
    static rhizo_ap_cr3: u8;
    static rhizo_ap_stack: u8;
    static rhizo_ap_percpu: u8;
    static rhizo_ap_index: u8;
    static rhizo_ap_entry: u8;
    static rhizo_ap_trampoline_end: u8;
}

macro_rules! offset {
    ($symbol:ident) => {
        // Only the addresses of the linker symbols are taken.
        core::ptr::addr_of!($symbol) as u64 - core::ptr::addr_of!(rhizo_ap_trampoline) as u64
    };
}

/// APIC identifier of each CPU, by number.
static IDS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Physical memory for the ACPI tables: through the direct map, mapping
/// read-only any page the direct map does not cover (tables may sit
/// outside RAM).
struct Firmware;

impl Firmware {
    fn ensure_mapped(phys: u64, len: u64) {
        let first = phys & !(FRAME_SIZE - 1);
        for page in (first..phys + len.max(1)).step_by(FRAME_SIZE as usize) {
            if memory::translate(DirectMap::virt(page)).is_none() {
                let _ = memory::map_page(DirectMap::virt(page), page, PageFlags::NO_EXECUTE);
            }
        }
    }
}

impl PhysicalMemory for Firmware {
    fn read_u64(&self, phys: u64) -> u64 {
        Self::ensure_mapped(phys, 8);
        DirectMap.read_u64(phys)
    }

    fn write_u64(&mut self, _: u64, _: u64) {
        unreachable!("firmware tables are only read");
    }

    fn read_bytes(&self, phys: u64, out: &mut [u8]) {
        Self::ensure_mapped(phys, out.len() as u64);
        DirectMap.read_bytes(phys, out);
    }
}

/// The APIC identifiers of the MADT (none without ACPI).
fn firmware_cpus() -> Vec<u64> {
    let start_info = super::boot::boot_arg();
    let hint = acpi::pvh_rsdp(&DirectMap, start_info);
    let mut ids = Vec::new();
    if let Some(rsdp) = acpi::find_rsdp(&Firmware, hint) {
        let _ = acpi::processors(&Firmware, rsdp, |id| ids.push(id as u64));
    }
    ids
}

/// Whether the trampoline page is RAM holding none of the boot information.
fn trampoline_page_is_free() -> bool {
    let Ok(info) = BootInfo::read(&DirectMap, super::boot::boot_arg()) else {
        return false;
    };
    let page = TRAMPOLINE..TRAMPOLINE + FRAME_SIZE;
    let overlaps = |(start, end): (u64, u64)| start < page.end && page.start < end;
    info.regions(&DirectMap)
        .any(|r| r.kind == RegionKind::Ram && r.start <= page.start && page.end <= r.end())
        && !info.occupied().into_iter().any(overlaps)
}

/// Busy-waits until `deadline` (monotonic ns) or `done`; returns `done`.
fn wait_until(deadline: u64, done: impl Fn() -> bool) -> bool {
    let now = || super::timer::cycles_to_ns(super::timer::Counter.cycles()).unwrap_or(0);
    while now() < deadline {
        if done() {
            return true;
        }
        core::hint::spin_loop();
    }
    done()
}

/// Starts the CPUs of the MADT, up to `limit` in all, one at a time;
/// returns how many CPUs are online.
///
/// # Panics
/// If a started CPU never reaches the kernel.
pub fn start(limit: Option<u16>) -> usize {
    let boot = irq::apic_id() as u64;
    IDS[0].store(boot, Ordering::Relaxed);
    let cpus = Cpus::select(boot, firmware_cpus(), limit);
    if cpus.len() == 1 || !irq::available() || !trampoline_page_is_free() {
        return 1;
    }
    // The temporary root: the kernel half, and the trampoline page where it
    // is, so the CPU keeps running when paging comes on. A handful of
    // frames, kept for good (CPUs start once).
    let kernel = memory::kernel_tables();
    let root = memory::with_frames(|f| {
        let pt = PageTable::<super::platform::Format>::new(&mut DirectMap, &mut || f.alloc())?;
        pt.share_upper_half(&mut DirectMap, &kernel);
        pt.map(
            &mut DirectMap,
            &mut || f.alloc(),
            TRAMPOLINE,
            TRAMPOLINE,
            PageSize::Small,
            PageFlags::empty(),
        )?;
        Ok::<_, rhizo_mm::paging::MapError>(pt.root())
    })
    .and_then(Result::ok)
    .filter(|&root| root < 1 << 32);
    let Some(root) = root else {
        return 1;
    };
    // SAFETY: the trampoline is a static byte range of the kernel image.
    let code = unsafe {
        let start = core::ptr::addr_of!(rhizo_ap_trampoline);
        let len = core::ptr::addr_of!(rhizo_ap_trampoline_end) as usize - start as usize;
        core::slice::from_raw_parts(start, len)
    };
    let mut mem = DirectMap;
    mem.write_bytes(TRAMPOLINE, code);
    let put32 = |mem: &mut DirectMap, at: u64, v: u64| {
        mem.write_bytes(TRAMPOLINE + at, &(v as u32).to_le_bytes())
    };
    let put64 =
        |mem: &mut DirectMap, at: u64, v: u64| mem.write_bytes(TRAMPOLINE + at, &v.to_le_bytes());
    put32(
        &mut mem,
        offset!(rhizo_ap_gdt_ptr) + 2,
        TRAMPOLINE + offset!(rhizo_ap_gdt),
    );
    put32(
        &mut mem,
        offset!(rhizo_ap_pm_target),
        TRAMPOLINE + offset!(rhizo_ap_pm),
    );
    put32(
        &mut mem,
        offset!(rhizo_ap_lm_target),
        TRAMPOLINE + offset!(rhizo_ap_lm),
    );
    put64(&mut mem, offset!(rhizo_ap_cr3), root);
    put64(
        &mut mem,
        offset!(rhizo_ap_entry),
        rhizo_ap_main as *const () as u64,
    );
    let mut online = 1;
    for &id in &cpus.ids()[1..] {
        let stacks = crate::smp::map_stacks(online);
        put64(&mut mem, offset!(rhizo_ap_stack), stacks.kernel);
        put64(&mut mem, offset!(rhizo_ap_percpu), percpu(online));
        put64(&mut mem, offset!(rhizo_ap_index), online as u64);
        IDS[online].store(id, Ordering::Relaxed);
        irq::send_init(id as u32);
        let now = super::timer::cycles_to_ns(super::timer::Counter.cycles()).unwrap_or(0);
        let vector = (TRAMPOLINE / FRAME_SIZE) as u8;
        irq::send_startup(id as u32, vector);
        if !wait_until(now + RETRY_NS, || crate::smp::arrived(online)) {
            irq::send_startup(id as u32, vector);
        }
        assert!(
            crate::smp::wait_arrival(online, ARRIVAL_NS),
            "CPU with APIC ID {id} was started but never reached the kernel"
        );
        online += 1;
    }
    online
}

/// First Rust code on a secondary CPU, on its own kernel stack, still on
/// the temporary root table.
extern "C" fn rhizo_ap_main(index: u64) -> ! {
    let index = index as usize;
    // SAFETY: the kernel's tables map this code, its stack and all kernel
    // data at the same addresses as the temporary root.
    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) memory::kernel_tables().root(), options(nostack, preserves_flags));
    }
    let stacks = crate::smp::stacks(index);
    cpu::init_cpu(stacks.double_fault, stacks.exception);
    irq::init_cpu();
    super::user::init_cpu();
    crate::smp::secondary_main(index)
}

/// Wakes CPU `cpu` from [`idle`].
pub fn wake(cpu: usize) {
    irq::wake(IDS[cpu].load(Ordering::Relaxed) as u32);
}

/// Sleeps until an interrupt (a wake-up) arrives.
pub fn idle() {
    irq::wait();
}
