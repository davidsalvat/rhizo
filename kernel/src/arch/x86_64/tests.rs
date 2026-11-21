//! x86_64-specific in-kernel tests.

use rhizo_core::ktest::TestCase;
use rhizo_core::ktest_case;

/// Architecture-specific cases, run after the shared suite.
pub static ALL: &[TestCase] = &[
    ktest_case!(pvh_start_info_is_valid),
    ktest_case!(kpti_leaves_cells_only_the_entry_pages),
    ktest_case!(user_guards_are_on),
    ktest_case!(a_cell_reads_the_counter),
    ktest_case!(the_timer_is_never_periodic_with_a_short_count),
    ktest_case!(the_clock_runs_at_the_apic_timer_s_rate),
    ktest_case!(the_x87_scrub_follows_the_cpu),
    ktest_case!(x87_pointers_never_survive_a_load),
];

/// The x87 pointers are scrubbed before each load exactly on AMD and Hygon
/// CPUs without `XSAVEERPTR` and under QEMU's emulation (design D1 of
/// `scrub-x87-pointers`); the test says which case ran, as only those CPUs
/// take the scrub's path.
fn the_x87_scrub_follows_the_cpu() {
    use core::arch::x86_64::__cpuid;
    use rhizo_hal::Console;
    let v = __cpuid(0);
    let mut vendor = [0u8; 12];
    for (i, word) in [v.ebx, v.edx, v.ecx].into_iter().enumerate() {
        vendor[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    let amd = &vendor == b"AuthenticAMD" || &vendor == b"HygonGenuine";
    let errptr = __cpuid(0x8000_0000).eax >= 0x8000_0008 && __cpuid(0x8000_0008).ebx & 4 != 0;
    let h = __cpuid(0x4000_0000);
    let mut hv = [0u8; 12];
    for (i, word) in [h.ebx, h.ecx, h.edx].into_iter().enumerate() {
        hv[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    let tcg = __cpuid(1).ecx >> 31 == 1 && &hv == b"TCGTCGTCGTCG";
    assert_eq!(super::user::fp_scrubs(), (amd && !errptr) || tcg);
    let note: &[u8] = if super::user::fp_scrubs() {
        b"(x87 scrub on) "
    } else {
        b"(x87 scrub off) "
    };
    super::console::BootConsole.write_bytes(note);
}

/// The x87 environment (`FNSTENV`, 32-bit format): the low halves of the
/// last instruction's address (word 3) and of its operand's (word 5).
fn x87_env() -> [u32; 7] {
    let mut env = [0u32; 7];
    // SAFETY: `env` is 28 writable bytes; the caller cleared CR0.TS (through
    // `fp_save`). `FNSTENV` only masks x87 exceptions, which the caller's
    // `fp_load` puts back.
    unsafe { core::arch::asm!("fnstenv [{}]", in(reg) &mut env, options(nostack)) };
    env
}

/// The x87 instruction and data pointers an x87 load leaves do not survive
/// loading another thread's FP/SIMD state, as a cell would read them next
/// (design D4 of `scrub-x87-pointers`). Where `FXRSTOR` restores them from
/// the image (Intel, Zen 2 and later) this holds without the scrub; on AMD
/// CPUs without `XSAVEERPTR` and under QEMU's emulation, which keeps them
/// whatever the image holds, only the scrub makes it hold.
/// A pointer the CPU does not record (FDP on Intel CPUs with
/// `FDP_EXCPTN_ONLY`) is not checked.
fn x87_pointers_never_survive_a_load() {
    use rhizo_cell::FpState;
    use rhizo_cell::elf::Machine;
    use rhizo_hal::Console;
    static MARK: u32 = 7;
    let mut saved = FpState::default();
    super::user::fp_save(&mut saved);
    let ip: u64;
    // SAFETY: CR0.TS is clear (`fp_save`) and the x87 stack holds what
    // `saved` keeps; `MARK` is a kernel static. `fninit` leaves no pending
    // exception and an empty stack, so the load cannot fault.
    unsafe {
        core::arch::asm!(
            "fninit",
            "lea {ip}, [rip + 2f]",
            "2: fild dword ptr [{mark}]",
            ip = out(reg) ip,
            mark = in(reg) &MARK,
            options(nostack, readonly)
        )
    };
    let marked = x87_env();
    let mark = &MARK as *const u32 as u64;
    let (fip, fdp) = (marked[3] == ip as u32, marked[5] == mark as u32);
    super::user::fp_load(&FpState::initial(Machine::X86_64));
    let after = x87_env();
    super::user::fp_load(&saved);
    if !fip && !fdp {
        super::console::BootConsole.write_bytes(b"(x87 pointers not recorded) ");
        return;
    }
    assert!(
        !fip || after[3] != ip as u32,
        "x87 instruction pointer kept: {ip:#x}"
    );
    assert!(
        !fdp || after[5] != mark as u32,
        "x87 data pointer kept: {mark:#x}"
    );
}

/// The TSC's conversion to time, however it was found (KVM's clock, CPUID,
/// the PIT or 10 ms of the APIC timer), agrees within 0.05% with 50 ms of
/// the local APIC timer, which counts at 1 GHz on KVM and QEMU (change
/// `fix-tcg-clock-rate`): under emulation the calibration made cells'
/// clocks 0.3 to 1.5% slow, up to a second per 65 s model call.
fn the_clock_runs_at_the_apic_timer_s_rate() {
    let hz = super::irq::tsc_hz_over(50_000_000).expect("the APIC timer runs");
    let second = super::timer::cycles_to_ns(hz).expect("the TSC frequency is known");
    assert!(
        second.abs_diff(1_000_000_000) < 500_000,
        "one second of the APIC timer reads as {second} ns ({hz} TSC cycles)"
    );
}

/// With KPTI (on for every in-kernel test), the kernel half of a cell's
/// tables maps the entry code and nothing of the kernel's memory: not the
/// direct map, not the heap, not the kernel's data or its page tables.
fn kpti_leaves_cells_only_the_entry_pages() {
    use crate::memory::{self, DirectMap};
    assert!(super::user::kpti(), "the test build turns KPTI on");
    let half = super::user::cell_kernel_half();
    let mapped = |virt: u64| half.translate(&DirectMap, virt).is_some();
    unsafe extern "C" {
        fn rhizo_syscall_entry_kpti();
    }
    let entry = rhizo_syscall_entry_kpti as *const () as u64;
    assert!(mapped(entry), "the system-call entry with KPTI is mapped");
    assert!(
        half.translate(&DirectMap, entry)
            .unwrap()
            .flags
            .is_executable(),
        "and executable"
    );
    use core::sync::atomic::AtomicU64;
    static PROBE: AtomicU64 = AtomicU64::new(1);
    let data = alloc::boxed::Box::new(0u64);
    for (what, virt) in [
        ("the direct map", DirectMap::virt(0x10_0000)),
        (
            "the kernel's tables",
            DirectMap::virt(memory::kernel_tables().root()),
        ),
        ("the heap", &*data as *const u64 as u64),
        (
            "kernel code",
            kpti_leaves_cells_only_the_entry_pages as *const () as u64,
        ),
        ("kernel data", &PROBE as *const AtomicU64 as u64),
    ] {
        assert!(!mapped(virt), "{what} ({virt:#x}) is mapped for cells");
    }
}

/// SMEP and SMAP are on wherever the CPU has them (QEMU's `max` CPU has
/// both); the self-checks `ktest-user-read` and `ktest-user-exec` show them
/// at work.
fn user_guards_are_on() {
    let guards = super::user::user_guards();
    assert_eq!(super::user::user_guards_on(), guards);
    if guards != 0b11 << 20 {
        use rhizo_hal::Console;
        super::console::BootConsole.write_bytes(b"(SMEP or SMAP missing on this CPU) ");
    }
}

fn pvh_start_info_is_valid() {
    super::boot::assert_boot_info_is_valid();
}

/// A cell reads the CPU's counter directly (`ticks`: `CNTVCT_EL0` and
/// `CNTFRQ_EL0` on AArch64, `rdtsc` on x86_64), without a trap killing it,
/// on whichever CPU it runs (change `add-node-workload`).
fn a_cell_reads_the_counter() {
    crate::tests::run_expected("ticks");
}

/// Across a wait and back to the periodic tick, the local APIC timer is
/// never periodic with a count shorter than the tick's, whatever the wait
/// was (design D1 of `fix-apic-tick-restart`): a periodic timer of one
/// count (16 ns) fired back to back under QEMU's lock and stopped every
/// CPU. The writes are replayed on a model of the timer's mode and count,
/// after each one.
fn the_timer_is_never_periodic_with_a_short_count() {
    use super::irq::{INITIAL_COUNT, LVT_TIMER, PERIODIC, tick_writes, wait_writes};
    const TICK: u32 = 625_000; // 10 ms
    // The periodic tick, as `init` leaves it.
    let (mut periodic, mut count) = (true, TICK);
    for ns in [0, 1, 15, 16, 17, 1_000, 9_999_999, u64::MAX] {
        for (reg, value) in wait_writes(ns).into_iter().chain(tick_writes(TICK)) {
            match reg {
                LVT_TIMER => periodic = value & PERIODIC != 0,
                INITIAL_COUNT => count = value,
                _ => panic!("unexpected register {reg:#x}"),
            }
            assert!(
                !periodic || count == TICK,
                "periodic with {count} counts after a wait of {ns} ns"
            );
        }
        assert!(periodic && count == TICK, "back to the tick after {ns} ns");
    }
}
