//! x86_64-specific in-kernel tests.

use rhizo_core::ktest::TestCase;
use rhizo_core::ktest_case;

/// Architecture-specific cases, run after the shared suite.
pub static ALL: &[TestCase] = &[
    ktest_case!(pvh_start_info_is_valid),
    ktest_case!(kpti_leaves_cells_only_the_entry_pages),
    ktest_case!(user_guards_are_on),
    ktest_case!(the_timer_is_never_periodic_with_a_short_count),
];

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
