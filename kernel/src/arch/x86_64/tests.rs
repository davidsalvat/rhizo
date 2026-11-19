//! x86_64-specific in-kernel tests.

use rhizo_core::ktest::TestCase;
use rhizo_core::ktest_case;

/// Architecture-specific cases, run after the shared suite.
pub static ALL: &[TestCase] = &[ktest_case!(pvh_start_info_is_valid)];

fn pvh_start_info_is_valid() {
    super::boot::assert_boot_info_is_valid();
}
