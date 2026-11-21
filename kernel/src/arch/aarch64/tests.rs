//! AArch64-specific in-kernel tests.

use rhizo_core::ktest::TestCase;
use rhizo_core::ktest_case;

/// Architecture-specific cases, run after the shared suite.
pub static ALL: &[TestCase] = &[
    ktest_case!(device_tree_is_valid),
    ktest_case!(counter_frequency_is_known),
    ktest_case!(pan_is_on),
    ktest_case!(a_cell_reads_the_counter),
];

/// PAN is on wherever the CPU has it (QEMU's `max` CPU does), also after
/// cells ran (every exception from EL0 sets it again); the self-check
/// `ktest-user-read` shows it at work.
fn pan_is_on() {
    if !super::user::has_pan() {
        use rhizo_hal::Console;
        super::console::BootConsole.write_bytes(b"(no PAN on this CPU) ");
        return;
    }
    assert!(super::user::pan_on(), "PSTATE.PAN is clear");
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

/// A cell reads the CPU's counter directly (`ticks`: `CNTVCT_EL0` and
/// `CNTFRQ_EL0` on AArch64, `rdtsc` on x86_64), without a trap killing it,
/// on whichever CPU it runs (change `add-node-workload`).
fn a_cell_reads_the_counter() {
    crate::tests::run_expected("ticks");
}
