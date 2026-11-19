//! In-kernel tests, run by `cargo xtask test` inside QEMU.

use crate::arch;
use rhizo_core::ktest::TestCase;
use rhizo_core::ktest_case;
use rhizo_hal::{Console, CycleCounter};

/// Builds the suite: the regular tests, followed by any extra cases.
macro_rules! suite {
    ($($extra:path),* $(,)?) => {
        &[
            ktest_case!(serial_output_works),
            ktest_case!(cycle_counter_is_monotonic),
            ktest_case!(boot_cycles_precede_now),
            ktest_case!(start_info_has_pvh_magic),
            ktest_case!(bss_starts_zeroed),
            $(ktest_case!($extra),)*
        ]
    };
}

/// Every in-kernel test case, in execution order.
#[cfg(not(any(feature = "ktest-should-panic", feature = "ktest-hang")))]
pub static ALL: &[TestCase] = suite!();

/// Harness self-check: the regular suite plus a test that panics.
#[cfg(feature = "ktest-should-panic")]
pub static ALL: &[TestCase] = suite!(deliberate_panic);

/// Harness self-check: the regular suite plus a test that never finishes.
#[cfg(feature = "ktest-hang")]
pub static ALL: &[TestCase] = suite!(hang_forever);

fn serial_output_works() {
    let mut serial = arch::serial::Serial::com1();
    serial.write_bytes(b"[serial] ");
}

fn cycle_counter_is_monotonic() {
    let tsc = arch::tsc::Tsc;
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
    assert!(arch::tsc::Tsc.cycles() >= entry);
}

fn start_info_has_pvh_magic() {
    let addr = arch::boot::start_info_addr() as usize;
    assert!(addr != 0, "no hvm_start_info pointer was recorded");
    // SAFETY: the first 1 GiB is identity-mapped and the hypervisor placed a
    // valid `hvm_start_info` (whose first field is the u32 magic) at `addr`.
    let magic = unsafe { core::ptr::read_volatile(addr as *const u32) };
    assert_eq!(magic, arch::boot::PVH_START_INFO_MAGIC);
}

fn bss_starts_zeroed() {
    use core::sync::atomic::{AtomicU64, Ordering};
    static ZEROED: AtomicU64 = AtomicU64::new(0);
    assert_eq!(ZEROED.load(Ordering::Relaxed), 0);
}

#[cfg(feature = "ktest-should-panic")]
fn deliberate_panic() {
    panic!("deliberate panic to check failure reporting");
}

#[cfg(feature = "ktest-hang")]
fn hang_forever() {
    loop {
        core::hint::spin_loop();
    }
}
