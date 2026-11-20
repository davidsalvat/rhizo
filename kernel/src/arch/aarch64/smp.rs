//! Starting the other CPUs with PSCI `CPU_ON` (spec `multi-core`): the
//! CPUs come from `/cpus` in the device tree, each enters
//! `rhizo_secondary_entry` (in `boot.rs`) with its start block.

use super::boot::layout;
use super::cpu::{EMPTY_ROOT, percpu};
use super::irq;
use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};
use rhizo_core::smp::{Cpus, MAX_CPUS};

const PSCI_CPU_ON: u64 = 0xc400_0003;
/// How long a started CPU may take to reach the kernel.
const ARRIVAL_NS: u64 = 2_000_000_000;

/// What `rhizo_secondary_entry` reads, at its physical address, before its
/// MMU is on: TCR, TTBR1, TTBR0, per-CPU data, kernel stack, exception
/// stack, index. One cache line, cleaned to memory before the CPU starts.
#[repr(C, align(64))]
struct Start([AtomicU64; 8]);

static START: Start = Start([const { AtomicU64::new(0) }; 8]);

/// Affinity (MPIDR) of each CPU, by number.
static IDS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

core::arch::global_asm!(
    r#"
    .text
    // Secondary CPUs (spec `multi-core`): PSCI CPU_ON enters here at the
    // physical address, MMU off, x0 = physical address of the start block
    // (`smp::Start`: TCR 0, TTBR1 8, TTBR0 16, per-CPU data 24, kernel
    // stack 32, exception stack 40, index 48). The boot identity map
    // carries it to the higher half, where it takes the kernel's tables.
    .global rhizo_secondary_entry
rhizo_secondary_entry:
    mov     x19, x0
    mrs     x0, CurrentEL
    lsr     x0, x0, #2
    cmp     x0, #2
    b.ne    2f
    mov     x0, #(1 << 31)
    msr     hcr_el2, x0
    mov     x0, #3
    msr     cnthctl_el2, x0
    msr     cntvoff_el2, xzr
    mov     x0, #0x3c5
    msr     spsr_el2, x0
    adr     x0, 2f
    msr     elr_el2, x0
    eret
2:  mrs     x0, CurrentEL
    lsr     x0, x0, #2
    cmp     x0, #1
    b.ne    9f
    msr     daifset, #0xf
    ldr     x0, =0x04ff
    msr     mair_el1, x0
    ldr     x0, [x19, #0]
    msr     tcr_el1, x0
    adrp    x0, rhizo_l0_low
    add     x0, x0, :lo12:rhizo_l0_low
    msr     ttbr0_el1, x0
    ldr     x0, [x19, #8]
    msr     ttbr1_el1, x0
    isb
    tlbi    vmalle1
    dsb     nsh
    isb
    mrs     x0, sctlr_el1
    mov     x1, #((1 << 0) | (1 << 2) | (1 << 12))
    orr     x0, x0, x1
    bic     x0, x0, #(1 << 1)
    msr     sctlr_el1, x0
    isb
    ldr     x0, =rhizo_secondary_high
    br      x0
9:  wfe
    b       9b

    .ltorg

rhizo_secondary_high:
    // Still reading the start block through the identity map.
    ldp     x20, x21, [x19, #16]    // TTBR0 (empty table), per-CPU data
    ldp     x22, x23, [x19, #32]    // kernel stack, exception stack
    ldr     x24, [x19, #48]         // index
    msr     spsel, #1
    mov     sp, x23
    msr     spsel, #0
    mov     sp, x22
    msr     tpidr_el1, x21
    ldr     x0, =rhizo_vectors
    msr     vbar_el1, x0
    msr     ttbr0_el1, x20          // drop the identity map
    isb
    tlbi    vmalle1
    dsb     nsh
    isb
    mov     x0, x24
    bl      rhizo_secondary_main
8:  wfe
    b       8b

    .ltorg
    "#
);

unsafe extern "C" {
    fn rhizo_secondary_entry();
}

/// Calls PSCI `CPU_ON` for the CPU `mpidr`, entering at physical `entry`
/// with `context` in x0, through the conduit the device tree names.
fn cpu_on(mpidr: u64, entry: u64, context: u64) -> i64 {
    let hvc = super::platform::device_tree().psci_method().ok().flatten() != Some(b"smc");
    let mut ret = PSCI_CPU_ON;
    // SAFETY: PSCI CPU_ON only starts another CPU at `entry`; the call
    // clobbers nothing beyond x0-x3 by the SMC calling convention.
    unsafe {
        if hvc {
            asm!("hvc #0", inout("x0") ret, in("x1") mpidr, in("x2") entry, in("x3") context, options(nostack));
        } else {
            asm!("smc #0", inout("x0") ret, in("x1") mpidr, in("x2") entry, in("x3") context, options(nostack));
        }
    }
    ret as i64
}

/// Starts the CPUs of the device tree that PSCI can start, up to `limit` in
/// all, one at a time; returns how many CPUs are online.
///
/// # Panics
/// If a CPU accepted by PSCI never reaches the kernel.
pub fn start(limit: Option<u16>) -> usize {
    let mut firmware = Vec::new();
    let _ = super::platform::device_tree().cpus(|c| {
        if c.psci {
            firmware.push(c.id);
        }
    });
    let boot = irq::mpidr();
    IDS[0].store(boot, Ordering::Relaxed);
    let cpus = Cpus::select(boot, firmware, limit);
    if cpus.len() == 1 {
        return 1;
    }
    irq::map_redistributors(cpus.len());
    let tcr: u64;
    let ttbr1: u64;
    // SAFETY: reading this CPU's translation control and kernel root.
    unsafe {
        asm!("mrs {}, tcr_el1", "mrs {}, ttbr1_el1", out(reg) tcr, out(reg) ttbr1, options(nomem, nostack));
    }
    let entry = layout::virt_to_phys(rhizo_secondary_entry as *const () as u64);
    let block = layout::virt_to_phys(&START as *const Start as u64);
    let mut online = 1;
    for &id in &cpus.ids()[1..] {
        let stacks = crate::smp::map_stacks(online);
        let values = [
            tcr,
            ttbr1,
            EMPTY_ROOT.load(Ordering::Relaxed),
            percpu(online, stacks.exception),
            stacks.kernel,
            stacks.exception,
            online as u64,
            0,
        ];
        for (slot, v) in START.0.iter().zip(values) {
            slot.store(v, Ordering::Relaxed);
        }
        // SAFETY: cleaning the start block's cache line to memory, where the
        // new CPU reads it with its MMU and caches off.
        unsafe { asm!("dc civac, {}", "dsb sy", in(reg) &START as *const Start, options(nostack)) };
        IDS[online].store(id, Ordering::Relaxed);
        if cpu_on(id, entry, block) != 0 {
            break; // PSCI refused: keep the CPUs started so far
        }
        assert!(
            crate::smp::wait_arrival(online, ARRIVAL_NS),
            "CPU {id:#x} was started but never reached the kernel"
        );
        online += 1;
    }
    online
}

/// First Rust code on a secondary CPU, on its own stacks.
#[unsafe(no_mangle)]
extern "C" fn rhizo_secondary_main(index: u64) -> ! {
    irq::init_cpu();
    crate::smp::secondary_main(index as usize)
}

/// Wakes CPU `cpu` from [`idle`].
pub fn wake(cpu: usize) {
    irq::wake(IDS[cpu].load(Ordering::Relaxed));
}

/// Sleeps until an interrupt (a wake-up) arrives.
pub fn idle() {
    irq::wait();
}
