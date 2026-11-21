//! Architecture-neutral in-kernel tests, run by `cargo xtask test` inside a
//! VM. Architecture-specific ones live in `arch/<cpu>/tests.rs`.
//!
//! Each area of the kernel has its file here, which lists its tests in its
//! own `ALL`, in execution order: a new test touches its area's file only.
//! The suite runs the areas in the order of [`AREAS`]. A test that takes 3
//! seconds or more on AArch64 (on 1 CPU plus on 4) has its cost there, in
//! tenths of a second (`ktest_case!(name, 120)`), so `RHIZO_SHARDS` splits
//! the suite evenly (change `shard-kernel-tests`).

use crate::arch;
use crate::cell::{Ending, Host, Pause, Resume};
use crate::memory::{self, DirectMap};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::alloc::Layout;
use rhizo_core::ktest::TestCase;
use rhizo_core::ktest_case;
use rhizo_hal::{Console, CycleCounter, PhysicalMemory};
use rhizo_snapshot::{FormatError, Snapshot};

use rhizo_mm::FRAME_SIZE;
use rhizo_mm::heap::TRIM_AT;
use rhizo_mm::paging::PageFlags;

mod boot;
mod cells;
mod cpython;
mod density;
mod dynamic;
mod files;
mod fork;
mod hierarchy;
mod images;
mod isolation;
mod jit;
mod leaks;
mod libc;
mod micropython;
mod mm;
mod net;
#[cfg(feature = "node")]
mod node;
#[cfg(feature = "packages")]
mod packages;
mod prof;
mod quotas;
mod shares;
mod smp;
mod snapshots;
mod threads;

use boot::*;
pub(crate) use cells::run_expected;
use cells::*;
use density::*;
use fork::*;
use threads::*;

/// The areas of the suite, in the order they run.
const AREAS: &[&[TestCase]] = &[
    boot::ALL,
    mm::ALL,
    cells::ALL,
    images::ALL,
    snapshots::ALL,
    fork::ALL,
    libc::ALL,
    files::ALL,
    density::ALL,
    isolation::ALL,
    quotas::ALL,
    leaks::ALL,
    hierarchy::ALL,
    shares::ALL,
    micropython::ALL,
    net::ALL,
    cpython::ALL,
    dynamic::ALL,
    threads::ALL,
    jit::ALL,
    smp::ALL,
    prof::ALL,
];

/// The regular suite: the tests of every area, area after area. In the
/// Node.js and package builds it is compiled but not run (it runs in the
/// plain `ktest` build).
#[cfg_attr(
    any(feature = "node", feature = "packages"),
    allow(dead_code, reason = "the suite runs in the regular `ktest` build")
)]
static SUITE: [TestCase; count(AREAS)] = concat(AREAS);

#[cfg(not(any(feature = "node", feature = "packages")))]
/// Every in-kernel test case, in execution order.
pub static ALL: &[TestCase] = &SUITE;

/// The Node.js tests (change `add-node-workload`), alone in the kernel
/// built with Node.js embedded, whose VM has the memory it needs.
#[cfg(feature = "node")]
pub static ALL: &[TestCase] = node::ALL;

/// The package tests (change `add-pip-packages`), alone in the kernel
/// built with the packages embedded, whose VM has the memory they need.
#[cfg(feature = "packages")]
pub static ALL: &[TestCase] = packages::ALL;

/// How many test cases `areas` hold in all.
const fn count(areas: &[&[TestCase]]) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < areas.len() {
        n += areas[i].len();
        i += 1;
    }
    n
}

/// The test cases of `areas`, area after area, each in its order.
const fn concat<const N: usize>(areas: &[&[TestCase]]) -> [TestCase; N] {
    let mut all = [areas[0][0]; N];
    let mut n = 0;
    let mut i = 0;
    while i < areas.len() {
        let mut j = 0;
        while j < areas[i].len() {
            all[n] = areas[i][j];
            n += 1;
            j += 1;
        }
        i += 1;
    }
    all
}

/// Builds a harness self-check: one passing case, then the case the harness
/// must catch. A self-check proves the harness, not the suite (which runs on
/// its own), so it stays short however long the suite grows.
macro_rules! self_check {
    ($extra:path) => {
        &[ktest_case!(console_output_works), ktest_case!($extra)]
    };
}

/// The harness self-checks, by the name `selfcheck=NAME` gives at boot
/// (spec `kernel-testing`): each proves that one kind of failure ends the
/// run and is reported, so the test build runs one instead of its suite.
static SELF_CHECKS: &[(&str, &[TestCase])] = &[
    // A test that panics.
    ("should-panic", self_check!(deliberate_panic)),
    // A test that never finishes.
    ("hang", self_check!(hang_forever)),
    // A write to address 0 must be reported as a page fault.
    ("page-fault", self_check!(write_to_null)),
    // Stack exhaustion must be reported as a double fault.
    ("stack-overflow", self_check!(overflow_the_stack)),
    // An invalid instruction must be reported.
    ("invalid-opcode", self_check!(execute_invalid_opcode)),
    // An impossible allocation must panic with its size.
    ("oom", self_check!(allocate_too_much)),
    // The kernel reading a user page must fault (SMAP, PAN).
    ("user-read", self_check!(kernel_reads_a_user_page)),
    // The kernel executing a user page must fault (SMEP, PXN).
    ("user-exec", self_check!(kernel_executes_a_user_page)),
];

/// The test cases this boot runs: the self-check `selfcheck=NAME` of the
/// command line `cmdline` (NUL-padded) names, otherwise [`ALL`]. An unknown
/// name fails the run.
pub fn chosen(cmdline: &[u8]) -> &'static [TestCase] {
    let Some(name) = rhizo_core::options::self_check(cmdline) else {
        return ALL;
    };
    match SELF_CHECKS.iter().find(|(n, _)| n.as_bytes() == name) {
        Some(&(_, cases)) => cases,
        None => panic!(
            "unknown self-check `{}`",
            core::str::from_utf8(name).unwrap_or("?")
        ),
    }
}

// --- Self-checks ------------------------------------------------------------

fn deliberate_panic() {
    panic!("deliberate panic to check failure reporting");
}

fn hang_forever() {
    loop {
        core::hint::spin_loop();
    }
}

/// Maps a page as a cell's (user, readable, executable, holding a `ret`)
/// at `0x40_0000` in a fresh lower half, loads that half, and returns the
/// page's address: what a stray kernel pointer into a cell would reach.
fn load_a_user_page() -> u64 {
    use crate::arch::platform::Format;
    use rhizo_mm::paging::{PageSize, PageTable};
    const PAGE: u64 = 0x40_0000;
    #[cfg(target_arch = "x86_64")]
    const RET: u64 = 0xc3;
    #[cfg(target_arch = "aarch64")]
    const RET: u64 = 0xd65f_03c0;
    let mut alloc = || memory::with_frames(|f| f.alloc()).flatten();
    let pt = PageTable::<Format>::new(&mut DirectMap, &mut alloc).expect("a root");
    if arch::user::CELL_ROOT_HAS_KERNEL_HALF {
        pt.share_upper_half(&mut DirectMap, &memory::kernel_tables());
    }
    let frame = alloc().expect("a frame");
    DirectMap.zero_frame(frame);
    DirectMap.write_u64(frame, RET);
    pt.map(
        &mut DirectMap,
        &mut alloc,
        PAGE,
        frame,
        PageSize::Small,
        PageFlags::USER,
    )
    .expect("map the page");
    arch::user::load_tables(pt.root());
    PAGE
}

fn kernel_reads_a_user_page() {
    let page = load_a_user_page() as *const u64;
    // SAFETY: deliberately outside the user-access helpers; the guard must
    // turn it into a fault that ends the run.
    let value = unsafe { page.read_volatile() };
    panic!("the kernel read a user page: {value:#x}");
}

fn kernel_executes_a_user_page() {
    let page = load_a_user_page();
    // SAFETY: deliberately invalid: the page holds a `ret`, which the guard
    // must refuse to fetch, ending the run.
    let f: extern "C" fn() = unsafe { core::mem::transmute(page as usize) };
    f();
    panic!("the kernel executed a user page");
}

fn write_to_null() {
    let null = core::hint::black_box(0usize) as *mut u64;
    // SAFETY: deliberately invalid; the resulting page fault ends the run.
    unsafe { null.write_volatile(1) };
}

fn overflow_the_stack() {
    #[allow(unconditional_recursion)]
    fn recurse(depth: u64) -> u64 {
        let frame = core::hint::black_box([depth as u8; 512]);
        recurse(depth + 1) + frame[0] as u64
    }
    core::hint::black_box(recurse(0));
}

fn execute_invalid_opcode() {
    arch::cpu::execute_invalid_instruction();
}

fn allocate_too_much() {
    let v: Vec<u8> = Vec::with_capacity(1 << 40);
    core::hint::black_box(v);
}

mod expected {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../cells/expected.rs"));
}
