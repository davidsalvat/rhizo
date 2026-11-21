//! x86_64-specific in-kernel tests.

use rhizo_core::ktest::TestCase;
use rhizo_core::ktest_case;

/// Architecture-specific cases, run after the shared suite.
pub static ALL: &[TestCase] = &[
    ktest_case!(pvh_start_info_is_valid),
    ktest_case!(user_guards_are_on),
    ktest_case!(the_timer_is_never_periodic_with_a_short_count),
];

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
