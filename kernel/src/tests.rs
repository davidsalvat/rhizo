//! Architecture-neutral in-kernel tests, run by `cargo xtask test` inside a
//! VM. Architecture-specific ones live in `arch/<cpu>/tests.rs`.

use crate::arch;
use crate::memory::{self, DirectMap};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::alloc::Layout;
use rhizo_core::ktest::TestCase;
use rhizo_core::ktest_case;
use rhizo_hal::{Console, CycleCounter, PhysicalMemory};

use rhizo_mm::FRAME_SIZE;
use rhizo_mm::paging::PageFlags;

/// Builds the suite: the regular tests, followed by any extra cases.
macro_rules! suite {
    ($($extra:path),* $(,)?) => {
        &[
            ktest_case!(console_output_works),
            ktest_case!(cycle_counter_is_monotonic),
            ktest_case!(boot_cycles_precede_now),
            ktest_case!(bss_starts_zeroed),
            ktest_case!(lower_half_is_unmapped),
            ktest_case!(kernel_code_is_executable_not_writable),
            ktest_case!(kernel_data_is_writable_not_executable),
            ktest_case!(stack_guard_page_is_unmapped),
            ktest_case!(direct_map_uses_largest_pages),
            ktest_case!(frames_are_unique_and_aligned),
            ktest_case!(contiguous_frames),
            ktest_case!(map_use_and_unmap_a_page),
            ktest_case!(double_mapping_is_rejected),
            ktest_case!(box_and_vec_work),
            ktest_case!(heap_alignment_is_honored),
            ktest_case!(heap_memory_is_reused),
            ktest_case!(cell_says_hello),
            ktest_case!(cell_exit_status_and_frames_return),
            ktest_case!(cell_system_calls),
            ktest_case!(cell_null_write_kills_only_the_cell),
            ktest_case!(cell_cannot_read_kernel_memory),
            ktest_case!(cell_rejects_bad_executables),
            $(ktest_case!($extra),)*
        ]
    };
}

#[cfg(not(any(
    feature = "ktest-should-panic",
    feature = "ktest-hang",
    feature = "ktest-page-fault",
    feature = "ktest-stack-overflow",
    feature = "ktest-invalid-opcode",
    feature = "ktest-oom",
)))]
/// Every in-kernel test case, in execution order.
pub static ALL: &[TestCase] = suite!();

/// Harness self-check: the regular suite plus a test that panics.
#[cfg(feature = "ktest-should-panic")]
pub static ALL: &[TestCase] = suite!(deliberate_panic);

/// Harness self-check: the regular suite plus a test that never finishes.
#[cfg(feature = "ktest-hang")]
pub static ALL: &[TestCase] = suite!(hang_forever);

/// Self-check: a write to address 0 must be reported as a page fault.
#[cfg(feature = "ktest-page-fault")]
pub static ALL: &[TestCase] = suite!(write_to_null);

/// Self-check: stack exhaustion must be reported as a double fault.
#[cfg(feature = "ktest-stack-overflow")]
pub static ALL: &[TestCase] = suite!(overflow_the_stack);

/// Self-check: an invalid instruction must be reported.
#[cfg(feature = "ktest-invalid-opcode")]
pub static ALL: &[TestCase] = suite!(execute_invalid_opcode);

/// Self-check: an impossible allocation must panic with its size.
#[cfg(feature = "ktest-oom")]
pub static ALL: &[TestCase] = suite!(allocate_too_much);

// --- H0 tests ---------------------------------------------------------------

fn console_output_works() {
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

// --- Virtual memory ---------------------------------------------------------

fn lower_half_is_unmapped() {
    assert_eq!(memory::translate(0), None);
    let (kernel_phys, _) = arch::boot::layout::kernel_phys();
    assert_eq!(
        memory::translate(kernel_phys),
        None,
        "identity map must be gone"
    );
    assert_eq!(memory::translate(0x0000_7fff_ffff_f000), None);
}

fn kernel_code_is_executable_not_writable() {
    let code = kernel_code_is_executable_not_writable as *const () as u64;
    let t = memory::translate(code).expect("kernel code is mapped");
    assert!(t.flags.is_executable());
    assert!(!t.flags.is_writable());
}

fn kernel_data_is_writable_not_executable() {
    use core::sync::atomic::{AtomicU64, Ordering};
    static DATA: AtomicU64 = AtomicU64::new(7);
    assert_eq!(DATA.load(Ordering::Relaxed), 7);
    let t = memory::translate(&DATA as *const _ as u64).expect("kernel data is mapped");
    assert!(t.flags.is_writable());
    assert!(!t.flags.is_executable());
}

fn stack_guard_page_is_unmapped() {
    let guard = arch::boot::layout::stack_guard();
    assert_eq!(memory::translate(guard), None);
    assert!(memory::translate(guard + FRAME_SIZE).is_some());
}

fn direct_map_uses_largest_pages() {
    use rhizo_mm::paging::PageSize;
    // Any RAM frame will do; RAM is not at the same addresses everywhere.
    let frame = memory::with_frames(|f| f.alloc())
        .flatten()
        .expect("free frame");
    let t = memory::translate(DirectMap::virt(frame) + 8).expect("direct map present");
    memory::with_frames(|f| f.free(frame));
    assert_eq!(t.phys, frame + 8);
    let expected = if arch::cpu::supports_1gib_pages() {
        PageSize::Huge
    } else {
        PageSize::Large
    };
    assert_eq!(t.size, expected);
    assert!(t.flags.is_writable() && !t.flags.is_executable());
}

// --- Physical memory --------------------------------------------------------

fn frames_are_unique_and_aligned() {
    let frames: Vec<u64> = (0..64)
        .map(|_| {
            memory::with_frames(|f| f.alloc())
                .flatten()
                .expect("free frame")
        })
        .collect();
    let (kernel_start, kernel_end) = arch::boot::layout::kernel_phys();
    for (i, &frame) in frames.iter().enumerate() {
        assert_eq!(frame % FRAME_SIZE, 0);
        assert!(frame >= 1024 * 1024, "first MiB must stay reserved");
        assert!(frame + FRAME_SIZE <= kernel_start || frame >= kernel_end);
        assert!(!frames[..i].contains(&frame), "frame returned twice");
    }
    for frame in frames {
        memory::with_frames(|f| f.free(frame));
    }
}

fn contiguous_frames() {
    let run = memory::with_frames(|f| f.alloc_contiguous(4))
        .flatten()
        .expect("free run");
    for i in 0..4 {
        assert!(memory::with_frames(|f| f.is_allocated(run + i * FRAME_SIZE)).unwrap());
    }
    memory::with_frames(|f| f.free_contiguous(run, 4));
}

const TEST_PAGE: u64 = 0xffff_c000_0000_0000;

fn map_use_and_unmap_a_page() {
    let frame = memory::with_frames(|f| f.alloc())
        .flatten()
        .expect("free frame");
    memory::map_page(
        TEST_PAGE,
        frame,
        PageFlags::WRITABLE | PageFlags::NO_EXECUTE,
    )
    .expect("map test page");

    // SAFETY: TEST_PAGE was just mapped writable to a frame we own.
    unsafe { (TEST_PAGE as *mut u64).write_volatile(0x5a5a_1234) };
    assert_eq!(DirectMap.read_u64(frame), 0x5a5a_1234);

    assert_eq!(memory::unmap_page(TEST_PAGE), Ok(frame));
    assert_eq!(memory::translate(TEST_PAGE), None);
    memory::with_frames(|f| f.free(frame));
}

fn double_mapping_is_rejected() {
    use rhizo_mm::paging::MapError;
    let frame = memory::with_frames(|f| f.alloc())
        .flatten()
        .expect("free frame");
    let flags = PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
    memory::map_page(TEST_PAGE, frame, flags).expect("map test page");
    assert_eq!(
        memory::map_page(TEST_PAGE, frame, flags),
        Err(MapError::AlreadyMapped(TEST_PAGE))
    );
    assert_eq!(memory::unmap_page(TEST_PAGE), Ok(frame));
    memory::with_frames(|f| f.free(frame));
}

// --- Heap -------------------------------------------------------------------

fn box_and_vec_work() {
    let boxed = Box::new(0xfeed_u64);
    assert_eq!(*boxed, 0xfeed);
    let v: Vec<u64> = (0..20_000).collect();
    assert_eq!(v.iter().sum::<u64>(), 19_999 * 20_000 / 2);
}

fn heap_alignment_is_honored() {
    for align in [1, 8, 64, 4096] {
        let layout = Layout::from_size_align(40, align).unwrap();
        // SAFETY: non-zero size; freed right below with the same layout.
        let p = unsafe { alloc::alloc::alloc(layout) };
        assert!(!p.is_null());
        assert_eq!(p as usize % align, 0, "alignment {align}");
        // SAFETY: allocated above with this layout.
        unsafe { alloc::alloc::dealloc(p, layout) };
    }
}

fn heap_memory_is_reused() {
    let first = Box::new([1u8; 200]);
    let addr = &*first as *const _ as usize;
    drop(first);
    let free_before = memory::free_bytes();
    let second = Box::new([2u8; 200]);
    assert_eq!(&*second as *const _ as usize, addr);
    assert_eq!(
        memory::free_bytes(),
        free_before,
        "reuse must not take new frames"
    );
}

// --- Self-checks ------------------------------------------------------------

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

#[cfg(feature = "ktest-page-fault")]
fn write_to_null() {
    let null = core::hint::black_box(0usize) as *mut u64;
    // SAFETY: deliberately invalid; the resulting page fault ends the run.
    unsafe { null.write_volatile(1) };
}

#[cfg(feature = "ktest-stack-overflow")]
fn overflow_the_stack() {
    #[allow(unconditional_recursion)]
    fn recurse(depth: u64) -> u64 {
        let frame = core::hint::black_box([depth as u8; 512]);
        recurse(depth + 1) + frame[0] as u64
    }
    core::hint::black_box(recurse(0));
}

#[cfg(feature = "ktest-invalid-opcode")]
fn execute_invalid_opcode() {
    arch::cpu::execute_invalid_instruction();
}

#[cfg(feature = "ktest-oom")]
fn allocate_too_much() {
    let v: Vec<u8> = Vec::with_capacity(1 << 40);
    core::hint::black_box(v);
}

mod expected {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../cells/expected.rs"));
}

/// Runs test cell `name` and checks its output and ending against
/// `cells/expected.rs`; returns everything it printed.
fn run_expected(name: &str) -> alloc::string::String {
    let &(_, stdout, ending) = expected::PROGRAMS
        .iter()
        .find(|p| p.0 == name)
        .unwrap_or_else(|| panic!("{name} is not in cells/expected.rs"));
    let mut out = Vec::new();
    let got = crate::cell::run(
        crate::cell::test_image(name),
        &[name.as_bytes()],
        Some(&mut out),
    )
    .unwrap_or_else(|e| panic!("cannot spawn {name}: {e:?} (cells are built by `cargo xtask`)"));
    let text = alloc::string::String::from_utf8(out).expect("cell output is UTF-8");
    match ending {
        expected::Ending::Exit(status) => {
            assert_eq!(
                got,
                crate::cell::Ending::Exit(status),
                "{name} output: {text:?}"
            );
            assert_eq!(text, stdout, "{name} output");
        }
        expected::Ending::Fault => {
            assert_eq!(got, crate::cell::Ending::Fault, "{name} output: {text:?}");
            assert!(text.starts_with(stdout), "{name} output: {text:?}");
        }
    }
    text
}

fn cell_says_hello() {
    run_expected("hello");
}

fn cell_exit_status_and_frames_return() {
    let before = memory::free_bytes();
    run_expected("exit3");
    run_expected("hello");
    assert_eq!(
        memory::free_bytes(),
        before,
        "every frame of the cells is returned"
    );
}

fn cell_system_calls() {
    let before = memory::free_bytes();
    run_expected("abi");
    assert_eq!(
        memory::free_bytes(),
        before,
        "brk and mmap memory is returned"
    );
}

fn cell_null_write_kills_only_the_cell() {
    let before = memory::free_bytes();
    let report = run_expected("fault_null");
    assert!(report.contains(" killed: "), "{report}");
    assert!(report.contains("address 0x0: write"), "{report}");
    assert!(report.contains("user mode"), "{report}");
    // The kernel keeps running cells after a fault.
    run_expected("hello");
}

fn cell_cannot_read_kernel_memory() {
    let report = run_expected("fault_kernel");
    assert!(
        report.contains("address 0xffff800000000000: read"),
        "{report}"
    );
}

fn cell_rejects_bad_executables() {
    use rhizo_cell::elf::ElfError;
    let mut other = crate::cell::test_image("hello").to_vec();
    other[18] ^= 1; // e_machine of another architecture
    let before = memory::free_bytes();
    assert_eq!(
        crate::cell::run(b"not an elf", &[], None),
        Err(ElfError::NotElf)
    );
    assert!(matches!(
        crate::cell::run(&other, &[], None),
        Err(ElfError::WrongMachine { .. })
    ));
    assert_eq!(memory::free_bytes(), before);
}
