//! AArch64-specific in-kernel tests.

use rhizo_core::ktest::TestCase;
use rhizo_core::ktest_case;

/// Architecture-specific cases, run after the shared suite.
pub static ALL: &[TestCase] = &[
    ktest_case!(device_tree_is_valid),
    ktest_case!(counter_frequency_is_known),
    ktest_case!(e0pd_is_on_where_present),
];

/// Where the CPU has FEAT_E0PD, `TCR_EL1.E0PD1` is set: cell accesses to
/// the kernel half fault in constant time (QEMU's `max` CPU has it).
fn e0pd_is_on_where_present() {
    let tcr: u64;
    // SAFETY: reading TCR_EL1 has no side effects.
    unsafe { core::arch::asm!("mrs {}, tcr_el1", out(reg) tcr, options(nomem, nostack)) };
    assert_eq!(super::user::kpti(), super::user::has_e0pd());
    if super::user::kpti() {
        assert_ne!(tcr & (1 << 56), 0, "TCR_EL1.E0PD1 is clear");
    } else {
        use rhizo_hal::Console;
        super::console::BootConsole.write_bytes(b"(no E0PD on this CPU) ");
    }
}

fn device_tree_is_valid() {
    let dt = super::platform::device_tree();
    let mut regions = 0;
    dt.memory_regions(|_| regions += 1)
        .expect("memory nodes are readable");
    assert!(regions > 0, "the device tree describes no memory");
    assert!(
        super::platform::uart().is_some(),
        "the device tree describes no UART"
    );
}

fn counter_frequency_is_known() {
    assert!(super::timer::frequency() > 0, "CNTFRQ_EL0 is zero");
}
