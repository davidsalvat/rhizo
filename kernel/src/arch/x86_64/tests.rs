//! x86_64-specific in-kernel tests.

use rhizo_core::ktest::TestCase;
use rhizo_core::ktest_case;

/// Architecture-specific cases, run after the shared suite.
pub static ALL: &[TestCase] = &[
    ktest_case!(pvh_start_info_is_valid),
    ktest_case!(kpti_leaves_cells_only_the_entry_pages),
    ktest_case!(user_guards_are_on),
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
        fn rhizo_syscall_entry();
    }
    let entry = rhizo_syscall_entry as *const () as u64;
    assert!(mapped(entry), "the system-call entry is mapped");
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
