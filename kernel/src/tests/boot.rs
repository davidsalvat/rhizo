//! Boot: the console, the cycle counter, `.bss`.

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(console_output_works),
    ktest_case!(cycle_counter_is_monotonic),
    ktest_case!(boot_cycles_precede_now),
    ktest_case!(bss_starts_zeroed),
];

pub(super) fn console_output_works() {
    arch::console::BootConsole.write_bytes(b"[console] ");
}

fn cycle_counter_is_monotonic() {
    let tsc = arch::timer::Counter;
    let mut previous = tsc.cycles();
    for _ in 0..1_000 {
        let now = tsc.cycles();
        assert!(now >= previous, "TSC went backwards: {previous} -> {now}");
        previous = now;
    }
}

fn boot_cycles_precede_now() {
    let entry = arch::boot::entry_cycles();
    assert!(entry > 0, "boot timestamp was not recorded");
    assert!(arch::timer::Counter.cycles() >= entry);
}

fn bss_starts_zeroed() {
    use core::sync::atomic::{AtomicU64, Ordering};
    static ZEROED: AtomicU64 = AtomicU64::new(0);
    assert_eq!(ZEROED.load(Ordering::Relaxed), 0);
}
