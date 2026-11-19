//! AArch64-specific in-kernel tests.

use rhizo_core::ktest::TestCase;
use rhizo_core::ktest_case;

/// Architecture-specific cases, run after the shared suite.
pub static ALL: &[TestCase] = &[
    ktest_case!(device_tree_is_valid),
    ktest_case!(counter_frequency_is_known),
];

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
