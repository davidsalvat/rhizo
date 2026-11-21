//! Performance counters (spec `profiling`): retired instructions and core
//! cycles, in kernel and cell code, on general-purpose counters 0 and 1.
//!
//! Intel's architectural performance monitoring (CPUID leaf 0xA) and AMD's
//! core counters (`PerfCtrExtCore`, CPUID 0x8000_0001) program the same way:
//! an event select register and a counter per counter. A hypervisor that
//! gives the guest no counters reports neither, and the kernel then measures
//! cycles with the time stamp counter instead.

use super::cpu::{rdmsr, wrmsr};
use core::arch::x86_64::__cpuid;
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// Event select: count in ring 3 (USR) and ring 0 (OS), enabled (EN).
const USR_OS_EN: u64 = (1 << 16) | (1 << 17) | (1 << 22);

/// The counters' registers, by vendor.
struct Msrs {
    select: u32,
    counter: u32,
    /// Distance between two counters' registers.
    stride: u32,
    instructions: u64,
    cycles: u64,
}

const INTEL: Msrs = Msrs {
    select: 0x186, // IA32_PERFEVTSEL0
    counter: 0xc1, // IA32_PMC0
    stride: 1,
    instructions: 0xc0, // INST_RETIRED.ANY_P
    cycles: 0x3c,       // CPU_CLK_UNHALTED.THREAD_P
};

const AMD: Msrs = Msrs {
    select: 0xc001_0200, // PERF_CTL0
    counter: 0xc001_0201,
    stride: 2,
    instructions: 0xc0, // retired instructions
    cycles: 0x76,       // cycles not halted
};

/// 0: not looked yet; 1: none; 2: Intel; 3: AMD.
static KIND: AtomicU8 = AtomicU8::new(0);

/// What the counters measure ([`init`]).
#[derive(Debug, Clone, Copy)]
pub struct Info {
    /// Who designed them.
    pub kind: &'static str,
    /// General-purpose counters per CPU.
    #[cfg_attr(
        not(feature = "bench-cell"),
        allow(dead_code, reason = "benchmark report")
    )]
    pub counters: u32,
    /// Whether it counts retired instructions (always, when found).
    #[cfg_attr(
        not(feature = "bench-cell"),
        allow(dead_code, reason = "benchmark report")
    )]
    pub instructions: bool,
}

fn detect() -> Option<(Info, &'static Msrs)> {
    let vendor = __cpuid(0);
    let amd = [vendor.ebx, vendor.edx, vendor.ecx] == [0x6874_7541, 0x6974_6e65, 0x444d_4163];
    if amd {
        let ext = __cpuid(0x8000_0000).eax;
        let core = ext >= 0x8000_0001 && __cpuid(0x8000_0001).ecx & (1 << 23) != 0;
        return core.then_some((
            Info {
                kind: "amd",
                counters: 6,
                instructions: true,
            },
            &AMD,
        ));
    }
    if vendor.eax < 0xa {
        return None;
    }
    let leaf = __cpuid(0xa);
    let version = leaf.eax & 0xff;
    let counters = (leaf.eax >> 8) & 0xff;
    // EAX[31:24] bits of EBX are valid; EBX bit 0 set: no core cycles,
    // bit 1 set: no instructions retired.
    let events = (leaf.eax >> 24) & 0xff;
    let missing = leaf.ebx & 0b11 != 0;
    (version >= 1 && counters >= 2 && events >= 2 && !missing).then_some((
        Info {
            kind: "intel",
            counters,
            instructions: true,
        },
        &INTEL,
    ))
}

/// Starts counting instructions and cycles on this CPU, if it has counters.
pub fn init() -> Option<Info> {
    let Some((info, msrs)) = detect() else {
        KIND.store(1, Ordering::Relaxed);
        return None;
    };
    // SAFETY: CPUID reported these counters; programming counters 0 and 1
    // only changes what they count, and nothing else in the kernel uses
    // them.
    unsafe {
        for (n, event) in [msrs.instructions, msrs.cycles].into_iter().enumerate() {
            wrmsr(msrs.select + n as u32 * msrs.stride, 0);
            wrmsr(msrs.counter + n as u32 * msrs.stride, 0);
            wrmsr(msrs.select + n as u32 * msrs.stride, USR_OS_EN | event);
        }
        if info.kind == "intel" && __cpuid(0xa).eax & 0xff >= 2 {
            // IA32_PERF_GLOBAL_CTRL: counters 0 and 1 on.
            wrmsr(0x38f, rdmsr(0x38f) | 0b11);
        }
        if info.kind == "amd"
            && __cpuid(0x8000_0000).eax >= 0x8000_0022
            && __cpuid(0x8000_0022).eax & 1 != 0
        {
            // PerfMonV2's PerfCntrGlobalCtl: counters 0 and 1 on.
            wrmsr(0xc000_0301, rdmsr(0xc000_0301) | 0b11);
        }
    }
    KIND.store(if info.kind == "amd" { 3 } else { 2 }, Ordering::Relaxed);
    Some(info)
}

/// Counts since [`init`]: retired instructions and core cycles.
#[derive(Debug, Clone, Copy)]
pub struct Counts {
    pub instructions: Option<u64>,
    pub cycles: u64,
}

impl Counts {
    /// What was counted from `earlier` to `self` (48-bit counters wrap).
    #[cfg_attr(not(feature = "ktest"), allow(dead_code, reason = "in-kernel tests"))]
    pub fn since(self, earlier: Counts) -> Counts {
        const MASK: u64 = (1 << 48) - 1;
        Counts {
            instructions: self
                .instructions
                .zip(earlier.instructions)
                .map(|(a, b)| a.wrapping_sub(b) & MASK),
            cycles: self.cycles.wrapping_sub(earlier.cycles) & MASK,
        }
    }
}

/// The counts now, if [`init`] found counters.
pub fn read() -> Option<Counts> {
    let msrs = match KIND.load(Ordering::Relaxed) {
        2 => &INTEL,
        3 => &AMD,
        _ => return None,
    };
    // SAFETY: `init` found and programmed these counters; reading them has
    // no side effects.
    let (instructions, cycles) =
        unsafe { (rdmsr(msrs.counter), rdmsr(msrs.counter + msrs.stride)) };
    Some(Counts {
        instructions: Some(instructions),
        cycles,
    })
}

/// Cycles between two samples, 0 while not sampling (spec `profiling`,
/// sampling).
static PERIOD: AtomicU64 = AtomicU64::new(0);

/// The vendor's counter registers, if [`init`] found counters.
fn msrs() -> Option<&'static Msrs> {
    match KIND.load(Ordering::Relaxed) {
        2 => Some(&INTEL),
        3 => Some(&AMD),
        _ => None,
    }
}

/// The global control and overflow-clear registers, where the counters
/// have them (Intel version 2 or later, AMD `PerfMonV2`).
fn global() -> Option<(u32, u32)> {
    match KIND.load(Ordering::Relaxed) {
        2 if __cpuid(0xa).eax & 0xff >= 2 => Some((0x38f, 0x390)),
        3 if __cpuid(0x8000_0000).eax >= 0x8000_0022 && __cpuid(0x8000_0022).eax & 1 != 0 => {
            Some((0xc000_0301, 0xc000_0303))
        }
        _ => None,
    }
}

/// Starts sampling on this CPU: counter 2 counts cycles in ring 3 only and
/// interrupts (local APIC `LVTPC`, [`super::irq::SAMPLE_VECTOR`]) when
/// `period` of them have passed, so the interrupt comes while cell code
/// runs. Returns whether there are counters for it ([`init`] found them).
#[cfg(feature = "sampling")]
pub fn start_sampling(period: u64) -> bool {
    let Some(m) = msrs() else {
        return false;
    };
    let enough = KIND.load(Ordering::Relaxed) == 3 || (__cpuid(0xa).eax >> 8) & 0xff >= 3;
    if !enough {
        return false;
    }
    PERIOD.store(period.clamp(1, i32::MAX as u64), Ordering::Relaxed);
    let select = m.select + 2 * m.stride;
    // SAFETY: CPUID reported counter 2; nothing else in the kernel uses it.
    // USR, INT (bit 20) and EN: cycles in ring 3, an interrupt on overflow.
    unsafe {
        wrmsr(select, 0);
        rearm();
        wrmsr(select, (1 << 16) | (1 << 20) | (1 << 22) | m.cycles);
        if let Some((ctrl, _)) = global() {
            wrmsr(ctrl, rdmsr(ctrl) | 0b100);
        }
    }
    super::irq::sample_interrupt(true);
    true
}

/// Stops sampling on this CPU.
#[cfg(feature = "sampling")]
pub fn stop_sampling() {
    if PERIOD.swap(0, Ordering::Relaxed) == 0 {
        return;
    }
    super::irq::sample_interrupt(false);
    if let Some(m) = msrs() {
        // SAFETY: counter 2 is the sampling counter `start_sampling` set.
        unsafe { wrmsr(m.select + 2 * m.stride, 0) };
    }
}

/// Clears the overflow, starts the next period and unmasks the interrupt
/// (Intel masks `LVTPC` when it delivers one), while sampling.
pub fn rearm() {
    let period = PERIOD.load(Ordering::Relaxed);
    let Some(m) = msrs().filter(|_| period != 0) else {
        return;
    };
    // SAFETY: counter 2 is the sampling counter; Intel sign-extends the
    // 32 bits written, AMD takes 48.
    unsafe {
        wrmsr(m.counter + 2 * m.stride, (1u64 << 48) - period);
        if let Some((_, clear)) = global() {
            wrmsr(clear, 0b100);
        }
    }
    super::irq::sample_interrupt(true);
}
