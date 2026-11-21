//! x86_64-specific in-kernel tests.

use rhizo_core::ktest::TestCase;
use rhizo_core::ktest_case;

/// Architecture-specific cases, run after the shared suite.
pub static ALL: &[TestCase] = &[
    ktest_case!(pvh_start_info_is_valid),
    ktest_case!(user_guards_are_on),
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
