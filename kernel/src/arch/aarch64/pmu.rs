//! Performance counters (spec `profiling`): PMUv3's cycle counter
//! (`PMCCNTR_EL0`) and event counter 0 on retired instructions
//! (`INST_RETIRED`, event 0x08), in EL0 and EL1.
//!
//! A hypervisor that gives the guest no PMU reports none in
//! `ID_AA64DFR0_EL1`, and the kernel then measures cycles with the generic
//! timer instead. QEMU's emulated PMU counts instructions only with
//! `-icount` (it says so in `PMCEID0_EL0`).

use core::arch::asm;
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// 0: not looked yet or none; 1: cycles only; 2: cycles and instructions,
/// 32-bit event counter; 3: with 64-bit event counters.
static KIND: AtomicU8 = AtomicU8::new(0);

/// What the counters measure ([`init`]).
#[derive(Debug, Clone, Copy)]
pub struct Info {
    /// Who designed them.
    #[cfg_attr(
        not(feature = "bench-cell"),
        allow(dead_code, reason = "benchmark report")
    )]
    pub kind: &'static str,
    /// Event counters per CPU (the cycle counter is extra).
    #[cfg_attr(
        not(feature = "bench-cell"),
        allow(dead_code, reason = "benchmark report")
    )]
    pub counters: u32,
    /// Whether it counts retired instructions.
    #[cfg_attr(
        not(feature = "bench-cell"),
        allow(dead_code, reason = "benchmark report")
    )]
    pub instructions: bool,
}

macro_rules! read_sys {
    ($reg:literal) => {{
        let value: u64;
        // SAFETY: callers read PMU registers only after `ID_AA64DFR0_EL1`
        // reported PMUv3; reads have no side effects.
        unsafe { asm!(concat!("mrs {}, ", $reg), out(reg) value, options(nomem, nostack)) };
        value
    }};
}

macro_rules! write_sys {
    ($reg:literal, $value:expr) => {{
        let value: u64 = $value;
        // SAFETY: callers write PMU registers only after `ID_AA64DFR0_EL1`
        // reported PMUv3; they only change what the counters count.
        unsafe { asm!(concat!("msr ", $reg, ", {}"), "isb", in(reg) value, options(nostack)) };
    }};
}

/// Starts counting cycles (and instructions, where the PMU counts them) on
/// this CPU, if it has a PMU.
pub fn init() -> Option<Info> {
    let version = (read_sys!("id_aa64dfr0_el1") >> 8) & 0xf;
    if version == 0 || version == 0xf {
        return None; // no PMU, or not PMUv3
    }
    let pmcr = read_sys!("pmcr_el0");
    let counters = ((pmcr >> 11) & 0x1f) as u32;
    let instructions = counters > 0 && read_sys!("pmceid0_el0") & (1 << 8) != 0;
    // PMUv3p5 (version 6) has 64-bit event counters (PMCR.LP).
    let long = version >= 6;
    // E: on; P, C: reset the counters; LC: 64-bit cycle counter.
    let lp = if long { 1 << 7 } else { 0 };
    write_sys!(
        "pmcr_el0",
        (pmcr & !0xff) | 1 | (1 << 1) | (1 << 2) | (1 << 6) | lp
    );
    write_sys!("pmccfiltr_el0", 0); // EL0 and EL1
    let mut enable = 1 << 31;
    if instructions {
        write_sys!("pmevtyper0_el0", 0x08); // INST_RETIRED, EL0 and EL1
        enable |= 1;
    }
    write_sys!("pmcntenset_el0", enable);
    let kind = match (instructions, long) {
        (false, _) => 1,
        (true, false) => 2,
        (true, true) => 3,
    };
    KIND.store(kind, Ordering::Relaxed);
    Some(Info {
        kind: "pmuv3",
        counters,
        instructions,
    })
}

/// Counts since [`init`]: retired instructions (where counted) and cycles.
#[derive(Debug, Clone, Copy)]
pub struct Counts {
    pub instructions: Option<u64>,
    pub cycles: u64,
}

impl Counts {
    /// What was counted from `earlier` to `self`.
    #[cfg_attr(not(feature = "ktest"), allow(dead_code, reason = "in-kernel tests"))]
    pub fn since(self, earlier: Counts) -> Counts {
        Counts {
            instructions: self
                .instructions
                .zip(earlier.instructions)
                .map(|(a, b)| a.wrapping_sub(b)),
            cycles: self.cycles.wrapping_sub(earlier.cycles),
        }
    }
}

/// A 32-bit event counter (before PMUv3p5) extended to 64 bits: its last
/// value and the wraps seen, on the CPU that reads it (the benchmarks'),
/// which reads it more often than every 2^32 instructions.
static LAST: AtomicU64 = AtomicU64::new(0);
static WRAPS: AtomicU64 = AtomicU64::new(0);

fn extend(low: u64) -> u64 {
    if low < LAST.swap(low, Ordering::Relaxed) {
        WRAPS.fetch_add(1, Ordering::Relaxed);
    }
    (WRAPS.load(Ordering::Relaxed) << 32) | low
}

/// The counts now, if [`init`] found a PMU.
pub fn read() -> Option<Counts> {
    let kind = KIND.load(Ordering::Relaxed);
    if kind == 0 {
        return None;
    }
    let cycles = read_sys!("pmccntr_el0");
    let instructions = match kind {
        2 => Some(extend(read_sys!("pmevcntr0_el0"))),
        3 => Some(read_sys!("pmevcntr0_el0")),
        _ => None,
    };
    Some(Counts {
        instructions,
        cycles,
    })
}
