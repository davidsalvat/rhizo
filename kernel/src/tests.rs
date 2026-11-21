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

/// Builds a harness self-check: one passing case, then the case the harness
/// must catch. A self-check proves the harness, not the suite (which runs on
/// its own), so it stays short however long the suite grows.
#[cfg(any(
    feature = "ktest-should-panic",
    feature = "ktest-hang",
    feature = "ktest-page-fault",
    feature = "ktest-stack-overflow",
    feature = "ktest-invalid-opcode",
    feature = "ktest-oom",
    feature = "ktest-user-read",
    feature = "ktest-user-exec",
))]
macro_rules! self_check {
    ($extra:path) => {
        &[ktest_case!(console_output_works), ktest_case!($extra)]
    };
}

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
            ktest_case!(large_heap_allocations_survive_fragmentation),
            ktest_case!(cell_says_hello),
            ktest_case!(cell_exit_status_and_frames_return),
            ktest_case!(cell_system_calls),
            ktest_case!(cell_null_write_kills_only_the_cell),
            ktest_case!(cell_cannot_read_kernel_memory),
            ktest_case!(cell_rejects_bad_executables),
            ktest_case!(cells_interleave_and_all_finish),
            ktest_case!(spinning_cell_does_not_starve_others),
            ktest_case!(sleeping_cell_uses_no_cpu_and_wakes_on_time),
            ktest_case!(same_address_is_private_to_each_cell),
            ktest_case!(cell_cannot_touch_another_cells_mapping),
            ktest_case!(fault_does_not_disturb_other_cells),
            ktest_case!(many_cells_in_sequence_stay_isolated),
            ktest_case!(an_image_spawned_again_starts_fresh),
            ktest_case!(kept_images_give_way_to_cells),
            ktest_case!(a_cell_from_a_kept_image_suspends_resumes_and_forks),
            ktest_case!(suspend_resume_after_every_step_is_invisible_eager),
            ktest_case!(suspend_resume_after_every_step_is_invisible_lazy),
            ktest_case!(suspend_between_parallel_runs_is_invisible),
            ktest_case!(suspend_returns_all_memory),
            ktest_case!(lazy_resume_restores_only_touched_pages),
            ktest_case!(suspended_cells_hold_no_kernel_resources),
            ktest_case!(snapshot_of_another_architecture_is_rejected),
            ktest_case!(forked_cells_see_only_their_own_writes),
            ktest_case!(children_outlive_their_parent_and_frames_return),
            ktest_case!(forks_of_forks),
            ktest_case!(fork_allocates_only_a_root),
            ktest_case!(spawn_from_template_runs_like_the_program),
            ktest_case!(real_libc_programs_behave_as_on_linux),
            ktest_case!(fp_state_is_private_to_each_cell),
            ktest_case!(fp_state_survives_suspend_and_resume),
            ktest_case!(warm_template_of_a_real_program),
            ktest_case!(files_behave_as_on_linux_and_return_memory),
            ktest_case!(files_are_private_to_each_cell),
            ktest_case!(initial_files_are_given_at_spawn),
            ktest_case!(open_file_survives_suspend_and_resume),
            ktest_case!(forked_cells_have_their_own_files),
            ktest_case!(template_files_are_inherited),
            ktest_case!(file_data_takes_its_own_size),
            ktest_case!(memory_pressure_fails_cells_not_the_kernel),
            ktest_case!(a_thousand_idle_cells),
            ktest_case!(memory_full_of_cells_is_refused_cleanly),
            ktest_case!(a_woken_cell_runs_at_once),
            ktest_case!(a_wake_before_the_sleep_is_kept),
            ktest_case!(hostile_pointers_get_efault),
            ktest_case!(cell_cannot_write_kernel_memory),
            ktest_case!(syscall_fuzzer_cannot_hurt_the_kernel),
            ktest_case!(syscall_storm_does_not_starve_others),
            ktest_case!(memory_hog_is_refused_and_others_run),
            ktest_case!(fresh_memory_never_holds_another_cells_data),
            ktest_case!(names_cannot_fill_kernel_memory),
            ktest_case!(shared_read_only_pages_stay_shared_after_mprotect),
            ktest_case!(cells_run_alike_with_and_without_kpti),
            ktest_case!(threads_racing_on_several_cpus_cannot_hurt_the_kernel),
            ktest_case!(flags_a_cell_sets_stay_in_the_cell),
            ktest_case!(one_page_mapped_without_end_cannot_stop_the_kernel),
            ktest_case!(hardware_entropy_works_where_present),
            ktest_case!(no_two_cells_draw_the_same_random_bytes),
            ktest_case!(micropython_in_a_cell),
            ktest_case!(sockets_and_pipes_behave_as_on_linux),
            ktest_case!(cells_talk_over_loopback),
            ktest_case!(a_waiting_cell_uses_no_cpu),
            ktest_case!(only_the_cells_waiting_on_a_socket_wake),
            ktest_case!(sockets_across_suspend_and_resume),
            ktest_case!(a_discarded_snapshot_closes_its_connections),
            ktest_case!(loopback_connection_across_suspend_of_one_end),
            ktest_case!(a_connection_to_the_host_survives_suspend),
            ktest_case!(connections_of_a_template_reset_in_its_cells),
            ktest_case!(a_blocked_cell_survives_suspend),
            ktest_case!(forked_servers_share_their_listener),
            ktest_case!(pipes_and_sockets_of_a_fork),
            ktest_case!(a_cell_reaches_the_host),
            ktest_case!(stack_grows_on_touch_and_returns_its_frames),
            ktest_case!(cpython_in_a_cell),
            ktest_case!(a_cpython_agent_talks_forks_and_survives_suspend),
            ktest_case!(dynamically_linked_programs_behave_as_on_linux),
            ktest_case!(a_dynamically_linked_cell_suspends_resumes_and_forks),
            ktest_case!(threads_behave_as_on_linux_and_return_memory),
            ktest_case!(spinning_thread_does_not_starve_its_siblings),
            ktest_case!(thread_count_does_not_buy_cpu_time),
            ktest_case!(cells_whose_threads_all_wait_use_no_cpu),
            ktest_case!(threaded_cell_survives_suspend_and_resume),
            ktest_case!(forks_of_a_threaded_cell_run_all_threads),
            ktest_case!(template_of_a_threaded_program),
            ktest_case!(every_cpu_has_its_number_and_own_stack),
            ktest_case!(kernel_globals_are_safe_across_cpus),
            ktest_case!(cells_run_on_every_cpu),
            ktest_case!(busy_cpus_do_not_starve_a_cell),
            ktest_case!(cells_in_parallel_stay_isolated),
            ktest_case!(unmapped_page_is_gone_on_every_cpu),
            ktest_case!(a_cell_on_several_cpus_survives_suspend_and_fork),
            ktest_case!(threads_of_a_cell_run_on_several_cpus_at_once),
            ktest_case!(performance_counters_count_where_present),
            ktest_case!(cost_accounting_adds_up_over_a_cell),
            ktest_case!(samples_show_where_a_cell_spins),
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
    feature = "ktest-user-read",
    feature = "ktest-user-exec",
)))]
/// Every in-kernel test case, in execution order.
pub static ALL: &[TestCase] = suite!();

/// In self-check builds the regular suite is compiled but not run (it runs
/// in the `ktest` build): referenced here so it keeps compiling everywhere.
#[cfg(any(
    feature = "ktest-should-panic",
    feature = "ktest-hang",
    feature = "ktest-page-fault",
    feature = "ktest-stack-overflow",
    feature = "ktest-invalid-opcode",
    feature = "ktest-oom",
    feature = "ktest-user-read",
    feature = "ktest-user-exec",
))]
#[allow(dead_code, reason = "the suite runs in the regular `ktest` build")]
static SUITE: &[TestCase] = suite!();

/// Harness self-check: a test that panics.
#[cfg(feature = "ktest-should-panic")]
pub static ALL: &[TestCase] = self_check!(deliberate_panic);

/// Harness self-check: a test that never finishes.
#[cfg(feature = "ktest-hang")]
pub static ALL: &[TestCase] = self_check!(hang_forever);

/// Self-check: a write to address 0 must be reported as a page fault.
#[cfg(feature = "ktest-page-fault")]
pub static ALL: &[TestCase] = self_check!(write_to_null);

/// Self-check: stack exhaustion must be reported as a double fault.
#[cfg(feature = "ktest-stack-overflow")]
pub static ALL: &[TestCase] = self_check!(overflow_the_stack);

/// Self-check: an invalid instruction must be reported.
#[cfg(feature = "ktest-invalid-opcode")]
pub static ALL: &[TestCase] = self_check!(execute_invalid_opcode);

/// Self-check: an impossible allocation must panic with its size.
#[cfg(feature = "ktest-oom")]
pub static ALL: &[TestCase] = self_check!(allocate_too_much);

/// Self-check: the kernel reading a user page must fault (SMAP, PAN).
#[cfg(feature = "ktest-user-read")]
pub static ALL: &[TestCase] = self_check!(kernel_reads_a_user_page);

/// Self-check: the kernel executing a user page must fault (SMEP, PXN).
#[cfg(feature = "ktest-user-exec")]
pub static ALL: &[TestCase] = self_check!(kernel_executes_a_user_page);

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

/// With no two free frames next to each other, a large heap allocation
/// still succeeds (mapped from single frames), holds what is written, and
/// gives every frame back.
fn large_heap_allocations_survive_fragmentation() {
    let before = memory::free_bytes();
    let mut taken: Vec<u64> = Vec::with_capacity((before / FRAME_SIZE) as usize);
    let mut kept = Vec::with_capacity(taken.capacity() / 2 + 1);
    while let Some(phys) = memory::with_frames(|f| f.alloc()).flatten() {
        taken.push(phys);
    }
    // Give back every other frame: plenty of memory, no run of two.
    taken.sort_unstable();
    for (i, phys) in taken.into_iter().enumerate() {
        if i % 2 == 0 {
            memory::with_frames(|f| f.free(phys));
        } else {
            kept.push(phys);
        }
    }
    let mut big = alloc::vec![0u64; (1 << 20) / 8];
    for (i, word) in big.iter_mut().enumerate() {
        *word = i as u64 * 3;
    }
    assert!(big.iter().enumerate().all(|(i, &w)| w == i as u64 * 3));
    drop(big);
    for phys in kept {
        memory::with_frames(|f| f.free(phys));
    }
    // Back, but for the window's page tables (made once).
    let lost = before - memory::free_bytes();
    assert!(lost <= 2 * FRAME_SIZE, "{lost} bytes not returned");
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

/// Maps a page as a cell's (user, readable, executable, holding a `ret`)
/// at `0x40_0000` in a fresh lower half, loads that half, and returns the
/// page's address: what a stray kernel pointer into a cell would reach.
#[cfg(any(feature = "ktest-user-read", feature = "ktest-user-exec"))]
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

#[cfg(feature = "ktest-user-read")]
fn kernel_reads_a_user_page() {
    let page = load_a_user_page() as *const u64;
    // SAFETY: deliberately outside the user-access helpers; the guard must
    // turn it into a fault that ends the run.
    let value = unsafe { page.read_volatile() };
    panic!("the kernel read a user page: {value:#x}");
}

#[cfg(feature = "ktest-user-exec")]
fn kernel_executes_a_user_page() {
    let page = load_a_user_page();
    // SAFETY: deliberately invalid: the page holds a `ret`, which the guard
    // must refuse to fetch, ending the run.
    let f: extern "C" fn() = unsafe { core::mem::transmute(page as usize) };
    f();
    panic!("the kernel executed a user page");
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
        .chain(expected::KERNEL_ONLY)
        .find(|p| p.0 == name)
        .unwrap_or_else(|| panic!("{name} is not in cells/expected.rs"));
    let (got, out) = crate::cell::run(crate::cell::test_image(name), &[name.as_bytes()])
        .unwrap_or_else(|e| {
            panic!("cannot spawn {name}: {e:?} (cells are built by `cargo xtask`)")
        });
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

/// Runs `name` once to warm the kernel heap, then checks that running it
/// again leaves free memory unchanged: every frame of the cell came back.
/// Memory is counted as [`memory_balance`] counts it, with the pages the
/// kernel heap holds, so the heap growing for something else meanwhile (the
/// network card answering, on another CPU) is not taken for a cell's leak.
fn assert_no_leak(name: &str) -> alloc::string::String {
    run_expected(name);
    let before = memory_balance().0;
    let text = run_expected(name);
    assert_eq!(
        memory_balance().0,
        before,
        "{name}: every frame is returned"
    );
    text
}

fn cell_exit_status_and_frames_return() {
    assert_no_leak("exit3");
    assert_no_leak("hello");
}

fn cell_system_calls() {
    assert_no_leak("abi");
}

fn cell_null_write_kills_only_the_cell() {
    let report = assert_no_leak("fault_null");
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
    // Spawned images are static (the kernel keeps them loaded by address).
    let other: &'static [u8] = other.leak();
    let before = memory::free_bytes();
    assert_eq!(
        crate::cell::run(b"not an elf", &[]).map(|r| r.0),
        Err(ElfError::NotElf)
    );
    assert!(matches!(
        crate::cell::run(other, &[]).map(|r| r.0),
        Err(ElfError::WrongMachine { .. })
    ));
    assert_eq!(memory::free_bytes(), before);
}

use crate::cell::{Ending, Host};

fn spawn(host: &mut Host, name: &str, args: &[&[u8]]) -> u32 {
    let mut argv = alloc::vec![name.as_bytes()];
    argv.extend_from_slice(args);
    host.spawn(crate::cell::test_image(name), &argv, true, true)
        .unwrap_or_else(|e| panic!("cannot spawn {name}: {e:?}"))
}

fn ended(host: &Host, id: u32) -> (Ending, alloc::string::String) {
    let f = host
        .finished(id)
        .unwrap_or_else(|| panic!("cell {id} has not ended"));
    (
        f.ending,
        alloc::string::String::from_utf8_lossy(&f.output).into_owned(),
    )
}

fn cells_interleave_and_all_finish() {
    let mut host = Host::new();
    let a = spawn(&mut host, "chatter", &[b"a"]);
    let b = spawn(&mut host, "chatter", &[b"b"]);
    host.run(|_| false);
    assert_eq!(ended(&host, a), (Ending::Exit(0), "a\na\na\n".into()));
    assert_eq!(ended(&host, b), (Ending::Exit(0), "b\nb\nb\n".into()));
}

fn spinning_cell_does_not_starve_others() {
    let mut host = Host::new();
    let spin = spawn(&mut host, "spin", &[]);
    let hello = spawn(&mut host, "hello", &[]);
    host.run(|h| h.finished(hello).is_some());
    assert_eq!(
        ended(&host, hello),
        (Ending::Exit(0), "hello from a cell\n".into())
    );
    assert_eq!(host.live(), 1, "the spinning cell is still running");
    assert!(host.kill(spin));
    assert_eq!(ended(&host, spin).0, Ending::Killed);
}

fn sleeping_cell_uses_no_cpu_and_wakes_on_time() {
    let mut host = Host::new();
    // Long enough that `chatter` finishes first even on a loaded host (the
    // guest clock follows the host's), so the order checked below does not
    // depend on how busy the machine running the tests is.
    let sleeper = spawn(&mut host, "sleeper", &[b"500"]);
    let chatter = spawn(&mut host, "chatter", &[]);
    host.run(|h| h.finished(chatter).is_some());
    assert!(
        host.finished(sleeper).is_none(),
        "the sleeper waits while others run"
    );
    host.run(|_| false);
    assert_eq!(
        ended(&host, sleeper),
        (Ending::Exit(0), "slept ok\n".into())
    );
}

fn same_address_is_private_to_each_cell() {
    let mut host = Host::new();
    let tags: [&[u8]; 8] = [b"0", b"1", b"2", b"3", b"4", b"5", b"6", b"7"];
    let ids: Vec<u32> = tags
        .iter()
        .map(|t| spawn(&mut host, "same_addr", &[t]))
        .collect();
    host.run(|_| false);
    for id in ids {
        assert_eq!(ended(&host, id), (Ending::Exit(0), "own value ok\n".into()));
    }
}

fn cell_cannot_touch_another_cells_mapping() {
    let mut host = Host::new();
    let mapper = spawn(&mut host, "mapper", &[]);
    while host.output(mapper) != Some(b"mapped\n") {
        host.step();
    }
    let peek = spawn(&mut host, "peek", &[]);
    host.run(|h| h.finished(peek).is_some());
    let (ending, report) = ended(&host, peek);
    assert_eq!(ending, Ending::Fault);
    assert!(report.contains("address 0x400000000000: read"), "{report}");
    assert!(host.kill(mapper), "the mapping cell is unaffected");
    assert_eq!(ended(&host, mapper), (Ending::Killed, "mapped\n".into()));
}

fn fault_does_not_disturb_other_cells() {
    let mut host = Host::new();
    let chatter = spawn(&mut host, "chatter", &[b"c"]);
    let fault = spawn(&mut host, "fault_null", &[]);
    host.run(|_| false);
    assert_eq!(ended(&host, fault).0, Ending::Fault);
    assert_eq!(ended(&host, chatter), (Ending::Exit(0), "c\nc\nc\n".into()));
}

/// More cells than 8-bit ASIDs, created and ended in pairs: identifiers are
/// reused and each cell still sees only its own memory; no frame is lost.
fn many_cells_in_sequence_stay_isolated() {
    fn pair() {
        let mut host = Host::new();
        let image = crate::cell::test_image("same_addr");
        let quiet = |host: &mut Host, tag: &[u8]| {
            host.spawn(image, &[b"same_addr", tag], true, false)
                .expect("spawn same_addr")
        };
        let a = quiet(&mut host, b"a");
        let b = quiet(&mut host, b"b");
        host.run(|_| false);
        for id in [a, b] {
            assert_eq!(ended(&host, id), (Ending::Exit(0), "own value ok\n".into()));
        }
    }
    pair();
    let before = memory::free_bytes();
    for _ in 0..150 {
        pair();
    }
    assert_eq!(memory::free_bytes(), before);
}

// --- Executables kept loaded (change `add-image-cache`) ---------------------

/// An executable spawned again, from the space kept for it, starts with the
/// data in the executable, not what an earlier cell of it wrote; two cells
/// of it at once keep their writes to themselves.
fn an_image_spawned_again_starts_fresh() {
    let mut host = Host::new();
    let first = spawn(&mut host, "same_addr", &[b"first"]);
    host.run(|_| false);
    assert_eq!(
        ended(&host, first),
        (Ending::Exit(0), "own value ok\n".into())
    );
    let ids = [b"a", b"b"].map(|tag| spawn(&mut host, "same_addr", &[tag]));
    host.run(|_| false);
    for id in ids {
        assert_eq!(ended(&host, id), (Ending::Exit(0), "own value ok\n".into()));
    }
}

/// Short of frames, the executables kept loaded are dropped before a cell
/// is refused: with every frame cells may take taken, but for those kept,
/// a new cell still starts and runs; nothing leaks.
fn kept_images_give_way_to_cells() {
    let before = memory_balance();
    run_expected("libc_hello"); // kept loaded from now on
    run_expected("libc_float");
    // The frames taken are listed in themselves: the heap stays as it is.
    let mut taken = u64::MAX;
    while let Some(frame) = memory::alloc_for_cell(1) {
        DirectMap.write_u64(frame, taken);
        taken = frame;
    }
    let ran = crate::cell::run(crate::cell::test_image("hello"), &[b"hello"]);
    while taken != u64::MAX {
        let next = DirectMap.read_u64(taken);
        memory::with_frames(|f| f.free_contiguous(taken, 1));
        taken = next;
    }
    let (ending, output) = ran.expect("the cell starts");
    assert_eq!(ending, Ending::Exit(0));
    assert_eq!(output, b"hello from a cell\n");
    drop(output);
    assert_eq!(memory_balance(), before, "nothing leaked");
}

/// A cell started from a kept executable is a cell like any other: it is
/// suspended, resumed and forked, and every copy runs to its end.
fn a_cell_from_a_kept_image_suspends_resumes_and_forks() {
    let mut host = Host::new();
    let first = forkme_ready(&mut host);
    assert!(host.kill(first)); // `forkme` is kept loaded from now on
    let parent = forkme_ready(&mut host);
    let snapshot = host.suspend(parent).expect("live");
    let resumed = host
        .resume(snapshot, Resume::Lazy, true, true)
        .expect("resume");
    let children = host.fork(resumed, 2).expect("fork");
    host.run(|_| false);
    for id in [resumed].into_iter().chain(children) {
        assert_eq!(ended(&host, id), (Ending::Exit(0), "own ok\n".into()));
    }
}

use crate::cell::Resume;
use rhizo_snapshot::{FormatError, Snapshot};

/// Runs `counter` to its end, suspending it after every scheduling step and
/// resuming it from the snapshot's bytes; its whole output must be the one
/// in `cells/expected.rs` (also Linux's, by the differential check).
fn suspend_resume_after_every_step(mode: Resume) {
    let expected = expected::PROGRAMS
        .iter()
        .find(|p| p.0 == "counter")
        .expect("counter")
        .1;
    let mut host = Host::new();
    let mut id = spawn(&mut host, "counter", &[]);
    let mut output = Vec::new();
    let mut resumes = 0;
    loop {
        host.step();
        if let Some(done) = host.finished(id) {
            assert_eq!(done.ending, Ending::Exit(0));
            output.extend_from_slice(&done.output);
            break;
        }
        let snapshot = host.suspend(id).expect("the counter is live");
        output.extend_from_slice(&host.finished(id).expect("suspended").output);
        let bytes = snapshot.into_bytes();
        let snapshot = Snapshot::from_bytes(bytes, crate::arch::user::MACHINE).expect("valid");
        id = host.resume(snapshot, mode, true, false).expect("resume");
        resumes += 1;
    }
    assert!(
        resumes >= 40,
        "suspended {resumes} times: at each of the 40 steps at least"
    );
    assert_eq!(alloc::string::String::from_utf8_lossy(&output), expected);
}

fn suspend_resume_after_every_step_is_invisible_eager() {
    suspend_resume_after_every_step(Resume::Eager);
}

fn suspend_resume_after_every_step_is_invisible_lazy() {
    suspend_resume_after_every_step(Resume::Lazy);
}

fn suspend_returns_all_memory() {
    fn cycle() -> Snapshot {
        let mut host = Host::new();
        let id = spawn(&mut host, "ballast", &[]);
        while host.output(id) != Some(b"filled\n") {
            host.step();
        }
        let snapshot = host.suspend(id).expect("live");
        assert_eq!(host.live(), 0);
        snapshot
    }
    drop(cycle()); // warm the heap up for the host's small allocations
    let before = memory::free_bytes();
    let snapshot = cycle();
    assert!(
        snapshot.as_bytes().len() > 8 << 20,
        "8 MiB of pages captured"
    );
    drop(snapshot);
    assert_eq!(
        memory::free_bytes(),
        before,
        "the suspended cell held nothing but its snapshot"
    );
}

fn lazy_resume_restores_only_touched_pages() {
    let mut host = Host::new();
    let id = spawn(&mut host, "ballast", &[b"one"]);
    while host.output(id) != Some(b"filled\n") {
        host.step();
    }
    let snapshot = host.suspend(id).expect("live");
    let pages = snapshot.pages();
    let id = host
        .resume(snapshot, Resume::Lazy, true, false)
        .expect("resume");
    assert_eq!(
        host.pending_pages(id),
        Some(pages),
        "nothing restored before it runs"
    );
    while host.output(id) != Some(b"checked\n") {
        host.step();
    }
    let left = host.pending_pages(id).expect("still live");
    assert!(
        left > 0 && left < pages,
        "{left} of {pages} pages still in the snapshot"
    );
    host.run(|_| false);
    assert_eq!(ended(&host, id), (Ending::Exit(0), "checked\n".into()));
}

fn suspended_cells_hold_no_kernel_resources() {
    let mut host = Host::new();
    let ids: Vec<u32> = (0..32).map(|_| spawn(&mut host, "chatter", &[])).collect();
    host.step();
    let snapshots: Vec<Snapshot> = ids
        .iter()
        .map(|&id| host.suspend(id).expect("live"))
        .collect();
    assert_eq!(
        host.live(),
        0,
        "no live cell, so no table, frame or tag held"
    );
    let resumed: Vec<u32> = snapshots
        .into_iter()
        .map(|s| host.resume(s, Resume::Lazy, true, false).expect("resume"))
        .collect();
    host.run(|_| false);
    for id in resumed {
        assert_eq!(host.finished(id).expect("ended").ending, Ending::Exit(0));
    }
}

fn snapshot_of_another_architecture_is_rejected() {
    let mut host = Host::new();
    let id = spawn(&mut host, "chatter", &[]);
    let mut bytes = host.suspend(id).expect("live").into_bytes();
    let other: u16 = if crate::arch::user::MACHINE as u16 == 183 {
        62
    } else {
        183
    };
    bytes[12..14].copy_from_slice(&other.to_le_bytes());
    assert!(matches!(
        Snapshot::from_bytes(bytes, crate::arch::user::MACHINE),
        Err(FormatError::Machine { .. })
    ));
}

/// Spawns `forkme` and runs it until it is ready to be forked.
fn forkme_ready(host: &mut Host) -> u32 {
    let id = spawn(host, "forkme", &[]);
    while host.output(id) != Some(b"ready\n") {
        host.step();
    }
    id
}

fn forked_cells_see_only_their_own_writes() {
    let mut host = Host::new();
    let parent = forkme_ready(&mut host);
    let children = host.fork(parent, 3).expect("fork");
    host.run(|_| false);
    assert_eq!(
        ended(&host, parent),
        (Ending::Exit(0), "ready\nown ok\n".into())
    );
    for child in children {
        // A child continues from the fork point: same output from there on.
        assert_eq!(ended(&host, child), (Ending::Exit(0), "own ok\n".into()));
    }
}

fn children_outlive_their_parent_and_frames_return() {
    fn round() {
        let mut host = Host::new();
        let parent = forkme_ready(&mut host);
        let children = host.fork(parent, 2).expect("fork");
        assert!(host.kill(parent));
        host.run(|_| false);
        for child in children {
            assert_eq!(ended(&host, child), (Ending::Exit(0), "own ok\n".into()));
        }
    }
    round();
    let before = memory::free_bytes();
    round();
    assert_eq!(
        memory::free_bytes(),
        before,
        "every shared frame and table returned"
    );
}

fn forks_of_forks() {
    let mut host = Host::new();
    let parent = forkme_ready(&mut host);
    let child = host.fork(parent, 1).expect("fork")[0];
    let grandchildren = host.fork(child, 2).expect("fork of a fork");
    host.run(|_| false);
    for id in [child].into_iter().chain(grandchildren) {
        assert_eq!(ended(&host, id), (Ending::Exit(0), "own ok\n".into()));
    }
}

fn fork_allocates_only_a_root() {
    let mut host = Host::new();
    let parent = spawn(&mut host, "ballast", &[b"all", b"8"]);
    while host.output(parent) != Some(b"filled\n") {
        host.step();
    }
    // Warm up: the same fork once, ended again, so the host's queues already
    // have room for the child and the heap does not grow during the measure.
    let warm = host.fork(parent, 1).expect("fork")[0];
    assert!(host.kill(warm));
    let before = memory::free_bytes();
    host.fork(parent, 1).expect("fork");
    assert_eq!(
        before - memory::free_bytes(),
        4096,
        "one root table, nothing else"
    );
}

fn spawn_from_template_runs_like_the_program() {
    let mut host = Host::new();
    let template = spawn(&mut host, "hello", &[]);
    assert!(host.freeze(template));
    let ids: Vec<u32> = (0..3)
        .map(|_| {
            host.spawn_from_template(template, true, true)
                .expect("spawn")
        })
        .collect();
    host.run(|_| false);
    for id in ids {
        assert_eq!(
            ended(&host, id),
            (Ending::Exit(0), "hello from a cell\n".into())
        );
    }
    assert!(host.finished(template).is_none(), "a template never runs");
}

/// Static glibc programs (cells/c): startup, stdio, the allocator, FP.
fn real_libc_programs_behave_as_on_linux() {
    for name in ["libc_hello", "libc_alloc", "libc_float"] {
        run_expected(name);
    }
}

/// Two FP programs holding values in FP/SIMD registers across yields, run
/// together: each must find its own values (lazy FP switching).
fn fp_state_is_private_to_each_cell() {
    let mut host = Host::new();
    let a = spawn(&mut host, "libc_float", &[b"1"]);
    let b = spawn(&mut host, "libc_float", &[b"1000"]);
    host.run(|_| false);
    assert_eq!(
        ended(&host, a),
        (Ending::Exit(0), "fp ok: 20100.0\n".into())
    );
    assert_eq!(
        ended(&host, b),
        (Ending::Exit(0), "fp ok: 219900.0\n".into())
    );
}

/// The FP state is part of the snapshot: suspended and resumed every few
/// steps (eagerly and lazily), next to another FP cell, the program still
/// gets its exact result.
fn fp_state_survives_suspend_and_resume() {
    for mode in [Resume::Eager, Resume::Lazy] {
        let mut host = Host::new();
        let other = spawn(&mut host, "libc_float", &[b"7"]);
        let mut id = spawn(&mut host, "libc_float", &[b"3"]);
        let mut output = Vec::new();
        for step in 1.. {
            host.step();
            if let Some(done) = host.finished(id) {
                assert_eq!(done.ending, Ending::Exit(0));
                output.extend_from_slice(&done.output);
                break;
            }
            if step % 9 == 0 {
                let snapshot = host.suspend(id).expect("live");
                output.extend_from_slice(&host.finished(id).expect("suspended").output);
                id = host.resume(snapshot, mode, true, true).expect("resume");
            }
        }
        assert_eq!(
            alloc::string::String::from_utf8_lossy(&output),
            "fp ok: 20500.0\n"
        );
        host.run(|_| false);
        assert_eq!(
            ended(&host, other),
            (Ending::Exit(0), "fp ok: 21300.0\n".into())
        );
    }
}

/// A real program frozen after its warm-up: every cell forked from it
/// answers without repeating the warm-up.
fn warm_template_of_a_real_program() {
    run_expected("libc_warm");
    let mut host = Host::new();
    let warm = spawn(&mut host, "libc_warm", &[]);
    while host.output(warm) != Some(b"warm\n") {
        host.step();
    }
    assert!(host.freeze(warm));
    let ids: Vec<u32> = (0..3)
        .map(|_| host.spawn_from_template(warm, true, true).expect("spawn"))
        .collect();
    host.run(|_| false);
    for id in ids {
        assert_eq!(
            ended(&host, id),
            (
                Ending::Exit(0),
                "primes below 2000000: 148933, below 1000: 168\n".into()
            )
        );
    }
}

// --- H8: files ---------------------------------------------------------------

/// A real program using files through glibc prints what it prints on Linux,
/// and an ended cell's files and descriptors return all their memory.
fn files_behave_as_on_linux_and_return_memory() {
    assert_no_leak("libc_files");
    assert_no_leak("diary");
}

fn files_are_private_to_each_cell() {
    let mut host = Host::new();
    let a = spawn(&mut host, "diary", &[b"first"]);
    let b = spawn(&mut host, "diary", &[b"second"]);
    host.run(|_| false);
    for id in [a, b] {
        // Both write `diary.txt`; each checks the file holds only its writes.
        assert_eq!(
            ended(&host, id),
            (Ending::Exit(0), "ready\nown ok\n".into())
        );
    }
}

fn initial_files_are_given_at_spawn() {
    let mut files = rhizo_fs::Fs::new();
    files
        .put_file(b"/input.txt", b"from the host")
        .expect("put");
    let mut host = Host::new();
    let id = host
        .spawn_with_files(
            crate::cell::test_image("diary"),
            &[b"diary"],
            files,
            true,
            true,
        )
        .expect("spawn");
    host.run(|_| false);
    assert_eq!(
        ended(&host, id),
        (
            Ending::Exit(0),
            "input from the host\nready\nown ok\n".into()
        )
    );
}

/// Suspended after every step with `diary.txt` open, and resumed from the
/// snapshot's bytes: the descriptor, its offset and the data all come back.
fn open_file_survives_suspend_and_resume() {
    for mode in [Resume::Eager, Resume::Lazy] {
        let mut host = Host::new();
        let mut id = spawn(&mut host, "diary", &[]);
        let mut output = Vec::new();
        let mut resumes = 0;
        loop {
            host.step();
            if let Some(done) = host.finished(id) {
                assert_eq!(done.ending, Ending::Exit(0));
                output.extend_from_slice(&done.output);
                break;
            }
            let snapshot = host.suspend(id).expect("live");
            output.extend_from_slice(&host.finished(id).expect("suspended").output);
            let snapshot = Snapshot::from_bytes(snapshot.into_bytes(), crate::arch::user::MACHINE)
                .expect("valid");
            id = host.resume(snapshot, mode, true, false).expect("resume");
            resumes += 1;
        }
        assert!(resumes >= 4, "suspended {resumes} times");
        assert_eq!(output, b"ready\nown ok\n");
    }
}

/// Spawns `diary` and runs it until its file is written and it yields.
fn diary_ready(host: &mut Host, tag: &[u8]) -> u32 {
    let id = spawn(host, "diary", &[tag]);
    while host.output(id) != Some(b"ready\n") {
        host.step();
    }
    id
}

fn forked_cells_have_their_own_files() {
    let mut host = Host::new();
    let parent = diary_ready(&mut host, b"shared");
    let children = host.fork(parent, 3).expect("fork");
    host.run(|_| false);
    assert_eq!(
        ended(&host, parent),
        (Ending::Exit(0), "ready\nown ok\n".into())
    );
    for child in children {
        // Each found the parent's `shared:` and only its own identifier.
        assert_eq!(ended(&host, child), (Ending::Exit(0), "own ok\n".into()));
    }
}

fn template_files_are_inherited() {
    let mut host = Host::new();
    let template = diary_ready(&mut host, b"template");
    assert!(host.freeze(template));
    let ids: Vec<u32> = (0..3)
        .map(|_| {
            host.spawn_from_template(template, true, true)
                .expect("spawn")
        })
        .collect();
    host.run(|_| false);
    for id in ids {
        assert_eq!(ended(&host, id), (Ending::Exit(0), "own ok\n".into()));
    }
}

/// A cell holding a 16 MiB file uses less than 17 MiB, its own memory
/// included: a 4 KiB block takes one page, not two.
fn file_data_takes_its_own_size() {
    let mut host = Host::new();
    let warm = spawn(&mut host, "filer", &[]);
    host.run(|h| h.finished(warm).is_some()); // the heap grows its classes once
    let before = memory::free_bytes();
    let id = spawn(&mut host, "filer", &[b"16"]);
    while host.output(id) != Some(b"files ok\n") {
        host.step();
    }
    let used = before - memory::free_bytes();
    assert!(used >= 16 << 20, "the file is held: {used} bytes");
    assert!(used < 17 << 20, "{used} bytes for 16 MiB of file data");
    host.run(|_| false);
}

// --- H9: memory accounting and density -------------------------------------

/// Memory that is not leaked: frames free in the allocator plus frames the
/// kernel heap holds (it keeps small-object pages for reuse, by design), and
/// the heap's live allocations; measured with no image kept loaded (spec
/// `cells`, image cache: those frames are the kernel's until dropped).
fn memory_balance() -> (u64, usize) {
    crate::cell::drop_images();
    let heap = crate::heap::stats();
    (
        memory::free_bytes() + heap.pages as u64 * FRAME_SIZE,
        heap.live,
    )
}

/// Twenty cells each wanting 8 MiB of memory, or 16 MiB of files, at once:
/// more than memory holds. Some are refused (`FAIL` from their own checks,
/// or a fault), the rest finish, the kernel keeps running, and nothing
/// leaks: every frame is back in the allocator or in the heap, and every
/// heap allocation is freed.
fn memory_pressure_fails_cells_not_the_kernel() {
    fn flood(name: &str, args: &[&[u8]]) -> (usize, usize) {
        let mut host = Host::new();
        let ids: Vec<u32> = (0..20).map(|_| spawn(&mut host, name, args)).collect();
        host.run(|_| false);
        let (mut done, mut refused) = (0, 0);
        for id in ids {
            match ended(&host, id) {
                (Ending::Exit(0), _) => done += 1,
                (Ending::Exit(1), out) if out.contains("FAIL") => refused += 1,
                (Ending::Fault, _) => refused += 1,
                other => panic!("{name}: unexpected ending {other:?}"),
            }
        }
        (done, refused)
    }
    for (name, args) in [
        // 16 MiB each: more than memory holds even when, on several
        // CPUs, some cells end before others start.
        ("ballast", &[&b"all"[..], b"16"][..]),
        ("filer", &[&b"16"[..]][..]),
    ] {
        let before = memory_balance();
        let (done, refused) = flood(name, args);
        assert!(
            done > 0 && refused > 0,
            "{name}: {done} finished, {refused} refused"
        );
        assert_eq!(memory_balance(), before, "{name}: nothing leaked");
    }
}

/// 1000 idle cells forked from one template, all live at once, each costing
/// a few pages beyond what it shares; killing them returns all of it.
fn a_thousand_idle_cells() {
    const N: usize = 1000;
    fn round() -> u64 {
        let mut host = Host::new();
        let template = spawn(&mut host, "idle", &[]);
        while host.output(template) != Some(b"idle\n") {
            host.step();
        }
        assert!(host.freeze(template));
        let before = memory::free_bytes();
        let ids: Vec<u32> = (0..N)
            .map(|_| {
                host.spawn_from_template(template, false, false)
                    .expect("fork")
            })
            .collect();
        // Each child runs once, into its sleep (one more step would wait an
        // hour for the first to wake).
        for _ in 0..N {
            host.step();
        }
        assert_eq!(host.live(), N);
        let per_cell = (before - memory::free_bytes()) / N as u64;
        for id in ids {
            assert!(host.kill(id));
        }
        per_cell
    }
    let before = memory_balance();
    let per_cell = round();
    assert!(per_cell < 64 << 10, "{per_cell} bytes per idle cell");
    assert_eq!(memory_balance(), before, "nothing leaked");
}

/// Idle cells forked until memory runs out (spec `benchmarks`, server
/// workload): the kernel refuses the next spawn, fork and eager resume, a
/// cell asking for memory gets `ENOMEM`, nothing panics, and every byte
/// comes back.
fn memory_full_of_cells_is_refused_cleanly() {
    let o = crate::workloads::fill_memory();
    assert!(o.cells > 1000, "{o:?}");
    assert!(
        o.spawn_refused && o.fork_refused && o.resume_refused && o.hog_enomem,
        "{o:?}"
    );
    assert_eq!(o.leaked_bytes, 0, "{o:?}");
}

/// A sleeping cell woken by the host (the stand-in for a user's message)
/// runs at once: `worker` sleeps an hour after `ready`, then works for one
/// second and exits.
fn a_woken_cell_runs_at_once() {
    let mut host = Host::new();
    let id = spawn(&mut host, "worker", &[b"cpu", b"10", b"1"]);
    while host.output(id) != Some(b"ready\n") {
        host.step();
    }
    assert!(host.wake(id));
    assert!(!host.wake(u32::MAX), "no such cell");
    host.run(|h| h.finished(id).is_some());
    let (ending, output) = ended(&host, id);
    assert_eq!(ending, Ending::Exit(0), "{output}");
    assert!(output.starts_with("ready\nops="), "{output}");
}

/// A wake from the host that comes while the cell is still on its way to
/// its sleep is kept for that sleep (design D5 of `add-socket-wait-queues`):
/// `a_woken_cell_runs_at_once` hung when a tick preempted the cell between
/// printing `ready` and sleeping, and the wake found nothing to end. Here
/// the cell yields between the two, so the wake always comes first.
fn a_wake_before_the_sleep_is_kept() {
    let mut host = Host::new();
    let id = spawn(&mut host, "sleeper", &[b"3600000", b"yield"]);
    while host.output(id) != Some(b"ready\n") {
        host.step();
    }
    // Before or at its yield: runnable, not asleep.
    assert!(host.wake(id));
    // At most its yield, then its sleep of an hour, which ends at once.
    for _ in 0..2 {
        if host.finished(id).is_none() {
            host.step();
        }
    }
    assert_eq!(ended(&host, id), (Ending::Exit(0), "ready\nwoken\n".into()));
}

// --- H13: isolation under attack (spec `isolation`) -------------------------

fn hostile_pointers_get_efault() {
    assert_no_leak("hostile_ptrs");
}

fn cell_cannot_write_kernel_memory() {
    let report = run_expected("poke_kernel");
    assert!(
        report.contains("address 0xffff800000000000: write"),
        "{report}"
    );
    run_expected("hello");
}

/// Twenty thousand random system calls next to a well-behaved cell: the
/// fuzzer gets only values and errnos, the other cell finishes, and running
/// it again leaves memory as it was.
fn syscall_fuzzer_cannot_hurt_the_kernel() {
    fn round() {
        let mut host = Host::new();
        let image = crate::cell::test_image("fuzz");
        let fuzz = host
            .spawn(image, &[b"fuzz"], true, false)
            .expect("spawn fuzz");
        let chatter = spawn(&mut host, "chatter", &[b"c"]);
        host.run(|_| false);
        assert_eq!(ended(&host, fuzz), (Ending::Exit(0), "fuzz ok\n".into()));
        assert_eq!(ended(&host, chatter), (Ending::Exit(0), "c\nc\nc\n".into()));
    }
    round();
    let before = memory_balance();
    round();
    assert_eq!(memory_balance(), before, "nothing leaked");
}

/// A cell that never stops making the calls that once took the kernel a
/// step per page of their range: the others still get the CPU.
fn syscall_storm_does_not_starve_others() {
    let mut host = Host::new();
    let storm = spawn(&mut host, "storm", &[]);
    let hello = spawn(&mut host, "hello", &[]);
    host.run(|h| h.finished(hello).is_some());
    assert_eq!(
        ended(&host, hello),
        (Ending::Exit(0), "hello from a cell\n".into())
    );
    assert_eq!(host.output(storm), Some(&b"storm\n"[..]));
    assert!(host.kill(storm));
}

/// A cell takes every frame it can: it is refused (`ENOMEM`), the cell
/// beside it finishes, and every frame comes back.
fn memory_hog_is_refused_and_others_run() {
    let before = memory_balance();
    let mut host = Host::new();
    let hog = spawn(&mut host, "hog", &[]);
    let chatter = spawn(&mut host, "chatter", &[b"c"]);
    host.run(|_| false);
    assert_eq!(ended(&host, hog), (Ending::Exit(0), "exhausted\n".into()));
    assert_eq!(ended(&host, chatter), (Ending::Exit(0), "c\nc\nc\n".into()));
    drop(host);
    assert_eq!(memory_balance(), before, "every frame came back");
    run_expected("hello");
}

/// Memory a cell is given never holds another cell's data: not when it
/// starts, not in a forked child, not after a resume (eager or lazy), each
/// right after another cell filled frames with a pattern and freed them.
/// And what an ended cell mapped is mapped for no one.
fn fresh_memory_never_holds_another_cells_data() {
    fn dirty(host: &mut Host) {
        let id = spawn(host, "scrub", &[b"dirty"]);
        host.run(|h| h.finished(id).is_some());
        assert_eq!(ended(host, id), (Ending::Exit(0), "dirty\n".into()));
    }
    fn ready(host: &mut Host) -> u32 {
        let id = spawn(host, "scrub", &[]);
        while host.output(id) != Some(b"ready\n") {
            host.step();
        }
        id
    }
    fn zero(host: &mut Host, ids: &[u32]) {
        host.run(|_| false);
        for &id in ids {
            let (ending, out) = ended(host, id);
            assert_eq!(ending, Ending::Exit(0), "{out}");
            assert!(out.ends_with("zero ok\n"), "{out}");
        }
    }
    let mut host = Host::new();
    dirty(&mut host);
    let fresh = ready(&mut host);
    zero(&mut host, &[fresh]);

    dirty(&mut host);
    let parent = ready(&mut host);
    let child = host.fork(parent, 1).expect("fork")[0];
    zero(&mut host, &[parent, child]);

    for mode in [Resume::Eager, Resume::Lazy] {
        let id = ready(&mut host);
        let snapshot = host.suspend(id).expect("suspend");
        dirty(&mut host);
        let resumed = host.resume(snapshot, mode, true, true).expect("resume");
        zero(&mut host, &[resumed]);
    }

    dirty(&mut host);
    let peek = spawn(&mut host, "peek", &[]);
    host.run(|_| false);
    let (ending, report) = ended(&host, peek);
    assert_eq!(ending, Ending::Fault, "the ended cell's mapping is gone");
    assert!(report.contains("address 0x400000000000: read"), "{report}");
}

/// Every other test runs with KPTI on (design D1 of `add-kpti`); the
/// kernel without it, as most CPUs run it, behaves the same: system calls,
/// faults, forks (copy-on-write flushes) and switches between cells.
fn cells_run_alike_with_and_without_kpti() {
    for kpti in [false, true] {
        arch::user::set_kpti(kpti);
        run_expected("abi");
        run_expected("fault_kernel");
        let mut host = Host::new();
        let parent = spawn(&mut host, "rewrite", &[]);
        let other = spawn(&mut host, "chatter", &[b"c"]);
        while host.output(parent) != Some(b"ready\n") {
            host.step();
        }
        let mut ids = host.fork(parent, 2).expect("fork");
        ids.push(parent);
        host.run(|_| false);
        for id in ids {
            let (ending, out) = ended(&host, id);
            assert_eq!(ending, Ending::Exit(0), "{out}");
            assert!(out.ends_with("own ok\n"), "{out}");
        }
        assert_eq!(ended(&host, other), (Ending::Exit(0), "c\nc\nc\n".into()));
    }
}

/// After a fork, read-only pages are shared: a cell that makes one writable
/// and writes it gets its own copy; the others keep theirs.
fn shared_read_only_pages_stay_shared_after_mprotect() {
    let mut host = Host::new();
    let parent = spawn(&mut host, "rewrite", &[]);
    while host.output(parent) != Some(b"ready\n") {
        host.step();
    }
    let mut ids = host.fork(parent, 2).expect("fork");
    ids.push(parent);
    host.run(|_| false);
    for id in ids {
        let (ending, out) = ended(&host, id);
        assert_eq!(ending, Ending::Exit(0), "{out}");
        assert!(out.ends_with("own ok\n"), "{out}");
    }
}

/// Hostile threads on several CPUs (spec `isolation`, `multi-core`):
/// `racer` cells unmap memory other threads are copying through the
/// kernel, unmap a word a thread waits on, end while their threads are in
/// system calls, and flip a page read-only under a writer. Each ends as its
/// race allows, a cell next to them finishes, the kernel keeps running and
/// every frame comes back.
fn threads_racing_on_several_cpus_cannot_hurt_the_kernel() {
    let before = memory_balance();
    let mut host = Host::new();
    let races = ["io", "futex", "exit", "protect"].map(|race| {
        let id = spawn(&mut host, "racer", &[race.as_bytes()]);
        (race, id)
    });
    let other = spawn(&mut host, "hello", &[]);
    host.run(|_| false);
    for (race, id) in races {
        let (ending, out) = ended(&host, id);
        match race {
            "exit" => assert_eq!(ending, Ending::Exit(3), "{race}: {out}"),
            // The writer may hit the page while it is read-only.
            "protect" if ending == Ending::Fault => {}
            _ => assert_eq!(
                (ending, out.as_str()),
                (Ending::Exit(0), alloc::format!("{race} ok\n").as_str()),
                "{race}"
            ),
        }
    }
    assert_eq!(ended(&host, other).0, Ending::Exit(0));
    drop(host);
    assert_eq!(memory_balance(), before, "every frame back");
}

/// A cell that spins with the direction and alignment-check flags set
/// (x86_64) is interrupted many times: the kernel's own code runs with
/// them clear, so its copies go forwards and SMAP stays on, and the cells
/// around it, which the kernel copies data for, are unharmed.
fn flags_a_cell_sets_stay_in_the_cell() {
    let mut host = Host::new();
    let flags = spawn(&mut host, "flags", &[]);
    let others: Vec<u32> = (0..3).map(|_| spawn(&mut host, "abi", &[])).collect();
    let before = memory::free_bytes();
    host.run(|_| false);
    assert_eq!(ended(&host, flags), (Ending::Exit(0), "flags ok\n".into()));
    for id in others {
        let (ending, out) = ended(&host, id);
        assert_eq!(
            (ending, out.as_str()),
            (Ending::Exit(0), expected_output("abi"))
        );
    }
    assert!(memory::free_bytes() >= before);
}

/// Finding S14 (spec `isolation`): a cell maps one page of a file of its
/// own 70000 times, more references to the page cache's frame than 16 bits
/// count. The kernel keeps running (it panicked), every mapping reads the
/// file, a cell next to it finishes, and every frame comes back.
fn one_page_mapped_without_end_cannot_stop_the_kernel() {
    let before = memory_balance();
    let mut host = Host::new();
    let sharer = spawn(&mut host, "sharer", &[]);
    let other = spawn(&mut host, "hello", &[]);
    host.run(|_| false);
    assert_eq!(
        ended(&host, sharer),
        (Ending::Exit(0), "shares ok\n".into())
    );
    assert_eq!(ended(&host, other).0, Ending::Exit(0));
    drop(host);
    assert_eq!(memory_balance(), before, "every frame back");
}

/// The CPU's random-number instruction, where it has one, gives bytes that
/// differ from one read to the next (QEMU's `max` CPU has it on both
/// architectures; without it the boot says `getrandom` is predictable).
fn hardware_entropy_works_where_present() {
    use rhizo_hal::Entropy;
    let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
    if !arch::random::HardwareEntropy.fill(&mut a) {
        arch::console::BootConsole.write_bytes(b"(no hardware random source on this CPU) ");
        return;
    }
    assert!(arch::random::HardwareEntropy.fill(&mut b));
    assert_ne!(a, b, "two reads gave the same bytes");
    assert_ne!(a, [0; 32]);
}

/// `getrandom` is a private stream per cell (spec `isolation`): independent
/// cells, forks, copies of a template and two resumes of one snapshot all
/// draw different bytes after the point they share.
fn no_two_cells_draw_the_same_random_bytes() {
    fn ready(host: &mut Host) -> u32 {
        let id = spawn(host, "dice", &[]);
        while host.output(id) != Some(b"ready\n") {
            host.step();
        }
        id
    }
    let mut host = Host::new();
    // Each is used as soon as it is ready (stepping may end the others).
    let mut ids = alloc::vec![ready(&mut host)];
    ids.extend(host.fork(ids[0], 2).expect("fork"));
    ids.push(ready(&mut host));
    let template = ready(&mut host);
    assert!(host.freeze(template));
    for _ in 0..2 {
        ids.push(
            host.spawn_from_template(template, true, false)
                .expect("copy"),
        );
    }
    let suspended = ready(&mut host);
    let bytes = host.suspend(suspended).expect("suspend").into_bytes();
    for mode in [Resume::Eager, Resume::Lazy] {
        let snapshot = Snapshot::from_bytes(bytes.clone(), arch::user::MACHINE).expect("valid");
        ids.push(host.resume(snapshot, mode, true, false).expect("resume"));
    }
    host.run(|_| false);
    let mut draws: Vec<alloc::string::String> = ids
        .iter()
        .map(|&id| {
            let (ending, out) = ended(&host, id);
            assert_eq!(ending, Ending::Exit(0), "{out}");
            out.trim_start_matches("ready\n").trim_end().into()
        })
        .collect();
    assert!(draws.iter().all(|d| d.len() == 32), "{draws:?}");
    draws.sort();
    draws.dedup();
    assert_eq!(draws.len(), ids.len(), "two cells drew the same bytes");
}

/// Cells make directories until refused. Next to a cell that takes all the
/// memory it can, names are refused once only the kernel's reserve is left
/// (names live in the kernel heap, which panics if it cannot grow); alone,
/// each cell stops at its own limit on files. All end, the kernel keeps
/// running, and every frame comes back.
fn names_cannot_fill_kernel_memory() {
    fn namers(host: &mut Host, n: usize) -> Vec<u32> {
        let image = crate::cell::test_image("namer");
        (0..n)
            .map(|_| {
                host.spawn(image, &[b"namer"], true, false)
                    .expect("spawn namer")
            })
            .collect()
    }
    let before = memory_balance();
    let mut host = Host::new();
    let mut ids = namers(&mut host, 4);
    let hog = spawn(&mut host, "hog", &[]);
    host.run(|_| false);
    assert_eq!(ended(&host, hog), (Ending::Exit(0), "exhausted\n".into()));
    ids.extend(namers(&mut host, 4));
    host.run(|_| false);
    for id in ids {
        assert_eq!(ended(&host, id), (Ending::Exit(0), "names full\n".into()));
    }
    drop(host);
    assert_eq!(memory_balance(), before, "every frame came back");
    run_expected("hello");
}

// --- MicroPython ----------------------------------------------------------

/// MicroPython running `cells/python/rhizo_demo.py`, imported from the
/// cell's own files, prints what it prints on Linux; frozen after its
/// warm-up as a template, every cell forked from it answers without warming
/// up again. Built only when its source is given (`RHIZO_MICROPYTHON_SRC`,
/// set in CI); otherwise the test says it did not run.
fn micropython_in_a_cell() {
    let image = crate::cell::test_image("micropython");
    if image.is_empty() {
        arch::console::BootConsole.write_bytes(b"(MicroPython not built: not run) ");
        return;
    }
    let files = || {
        let mut fs = rhizo_fs::Fs::new();
        fs.put_file(
            b"/lib/rhizo_demo.py",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../cells/python/rhizo_demo.py"
            )),
        )
        .expect("module");
        fs
    };
    let argv: [&[u8]; 3] = [b"micropython", b"-c", expected::PYTHON_COMMAND.as_bytes()];
    let mut host = Host::new();
    let id = host
        .spawn_with_files(image, &argv, files(), true, true)
        .expect("spawn");
    host.run(|_| false);
    assert_eq!(
        ended(&host, id),
        (Ending::Exit(0), expected::PYTHON_STDOUT.into())
    );
    let template = host
        .spawn_with_files(image, &argv, files(), true, false)
        .expect("spawn");
    while host.output(template) != Some(b"warm\n") {
        host.step();
    }
    assert!(host.freeze(template));
    let answer = expected::PYTHON_STDOUT
        .strip_prefix("warm\n")
        .expect("warm first");
    let ids: Vec<u32> = (0..3)
        .map(|_| {
            host.spawn_from_template(template, true, true)
                .expect("fork")
        })
        .collect();
    host.run(|_| false);
    for id in ids {
        assert_eq!(ended(&host, id), (Ending::Exit(0), answer.into()));
    }
}

/// Pipes, event counters, `poll`, `epoll`, `select` and sockets through a
/// real libc print what they print on Linux; every frame comes back.
fn sockets_and_pipes_behave_as_on_linux() {
    assert_no_leak("libc_net");
    for name in ["loopback", "piper", "netstate"] {
        run_expected(name);
    }
    assert_eq!(crate::cell::open_sockets(), 0, "every socket closed");
}

/// A server cell and a client cell exchange messages over TCP on 127.0.0.1.
fn cells_talk_over_loopback() {
    let mut host = Host::new();
    let server = spawn(&mut host, "loopback", &[b"serve", b"7300", b"100"]);
    let client = spawn(&mut host, "loopback", &[b"ping", b"7300", b"100"]);
    host.run(|_| false);
    assert_eq!(ended(&host, server), (Ending::Exit(0), "served\n".into()));
    assert_eq!(ended(&host, client), (Ending::Exit(0), "pinged\n".into()));
    assert_eq!(crate::cell::open_sockets(), 0);
}

/// A server waiting in `accept` does not run while another cell works; it
/// runs once a client connects.
fn a_waiting_cell_uses_no_cpu() {
    let mut host = Host::new();
    let server = spawn(&mut host, "loopback", &[b"serve", b"7301", b"1"]);
    host.step(); // it reaches `accept` and waits
    let chatter = spawn(&mut host, "chatter", &[]);
    host.run(|h| h.finished(chatter).is_some());
    assert!(host.finished(server).is_none(), "the server still waits");
    assert_eq!(host.waiting(), 1, "and it is not runnable");
    let client = spawn(&mut host, "loopback", &[b"ping", b"7301", b"1"]);
    host.run(|_| false);
    assert_eq!(ended(&host, server), (Ending::Exit(0), "served\n".into()));
    assert_eq!(ended(&host, client).0, Ending::Exit(0));
}

/// A socket change wakes only the cells waiting on that socket (design D1
/// of `add-socket-wait-queues`): a server waiting for a client that never
/// comes stays asleep while two other cells talk, and no wake-up finds
/// nothing to do.
fn only_the_cells_waiting_on_a_socket_wake() {
    use crate::prof::{self, Count};
    let mut host = Host::new();
    let idle = spawn(&mut host, "loopback", &[b"serve", b"7310", b"1"]);
    step_until_waiting(&mut host); // it reaches `accept` and waits
    prof::start();
    let server = spawn(&mut host, "loopback", &[b"serve", b"7311", b"50"]);
    let client = spawn(&mut host, "loopback", &[b"ping", b"7311", b"50"]);
    host.run(|h| h.finished(server).is_some() && h.finished(client).is_some());
    let report = prof::stop();
    assert_eq!(ended(&host, server), (Ending::Exit(0), "served\n".into()));
    assert_eq!(ended(&host, client), (Ending::Exit(0), "pinged\n".into()));
    assert_eq!(report.count(Count::IoWasted), 0, "no wake-up found nothing");
    assert!(host.finished(idle).is_none());
    assert_eq!(host.waiting(), 1, "the idle server was never woken");
    assert!(host.kill(idle));
    assert_eq!(crate::cell::open_sockets(), 0);
}

/// Runs `netstate` to its `ready` line.
fn netstate_ready(host: &mut Host) -> u32 {
    let id = spawn(host, "netstate", &[]);
    while host.output(id) != Some(b"ready\n") {
        host.step();
    }
    id
}

// --- CPython ---------------------------------------------------------------

/// A program using 4 MiB of stack, and a system call writing into stack it
/// never touched, run as on Linux: the stack grows a page at a time up to
/// its 8 MiB limit, and every page comes back when the cell ends (design D5
/// of `add-cpython`).
fn stack_grows_on_touch_and_returns_its_frames() {
    assert_no_leak("libc_deep_stack");
}

/// CPython running `cells/python/agent_demo.py` with its standard library
/// in the cell's files prints what it prints on Linux; frozen after its
/// warm-up as a template, every cell forked from it answers without warming
/// up again. Built only when its source is given (`RHIZO_CPYTHON_SRC`, set
/// in CI); otherwise the test says it did not run.
fn cpython_in_a_cell() {
    let image = crate::cell::test_image("cpython");
    let Some(files) = crate::cell::cpython_files() else {
        arch::console::BootConsole.write_bytes(b"(CPython not built: not run) ");
        return;
    };
    let argv = expected::CPYTHON_ARGV.map(str::as_bytes);
    let mut host = Host::new();
    let id = host
        .spawn_with_files(image, &argv, files.clone(), true, true)
        .expect("spawn");
    host.run(|_| false);
    assert_eq!(
        ended(&host, id),
        (Ending::Exit(0), expected::CPYTHON_STDOUT.into())
    );
    let template = host
        .spawn_with_files(image, &argv, files, true, false)
        .expect("spawn");
    while host.output(template) != Some(b"warm\n") {
        host.step();
    }
    assert!(host.freeze(template));
    let answer = expected::CPYTHON_STDOUT
        .strip_prefix("warm\n")
        .expect("warm first");
    let ids: Vec<u32> = (0..3)
        .map(|_| {
            host.spawn_from_template(template, true, true)
                .expect("fork")
        })
        .collect();
    host.run(|_| false);
    for id in ids {
        assert_eq!(ended(&host, id), (Ending::Exit(0), answer.into()));
    }
}

/// The CPython agent end to end (change `add-agent-workload`): it talks to
/// the host's fake model over HTTP through the network card for a few
/// turns, suspended and lazily resumed after each one with its connection
/// kept, then forks into branches at its decision point. Its transcript is
/// the one a Linux process prints (`cells/expected.rs`, checked on Linux by
/// `xtask`), and nothing stays open once the host is gone.
fn a_cpython_agent_talks_forks_and_survives_suspend() {
    if crate::cell::cpython_files().is_none() {
        arch::console::BootConsole.write_bytes(b"(CPython not built: not run) ");
        return;
    }
    if crate::wire::options().llm.is_none() {
        arch::console::BootConsole.write_bytes(b"(no network card and model server: not run) ");
        return;
    }
    let mut host = Host::new();
    let run = crate::cell::agent(
        &mut host,
        expected::AGENT_TURNS,
        Some(Resume::Lazy),
        b"fake",
        crate::cell::AgentOutput::Transcript,
    )
    .unwrap_or_else(|why| panic!("the agent did not finish: {why}"));
    assert_eq!(
        run.suspend_resume_ns.len(),
        expected::AGENT_TURNS,
        "suspended after every turn"
    );
    assert_eq!(
        alloc::string::String::from_utf8_lossy(&run.transcript),
        expected::AGENT_TRANSCRIPT
    );
    drop(host);
    assert_eq!(crate::cell::open_sockets(), 0);
    // The card's connections finish closing with the model server: the
    // kernel goes back to holding nothing for the agent.
    let mut host = Host::new();
    let end = crate::cell::now_ns() + 10_000_000_000;
    while crate::cell::wire_busy() && crate::cell::now_ns() < end {
        host.wire_address(0);
    }
    assert!(!crate::cell::wire_busy(), "the card's connections closed");
}

// --- C.1: dynamic linking ---------------------------------------------------

/// Spawns the program at `path` in `files` with `argv` (a
/// [`expected::DYNAMIC`] entry).
fn spawn_dynamic(host: &mut Host, files: &rhizo_fs::Fs, path: &str, argv: &[&str]) -> u32 {
    let argv: Vec<&[u8]> = argv.iter().map(|a| a.as_bytes()).collect();
    host.spawn_file(files.clone(), path.as_bytes(), &argv, &[], true, true)
        .unwrap_or_else(|e| panic!("cannot spawn {path}: {e:?}"))
}

/// The dynamically linked programs `xtask` packs (spec `cells`,
/// dynamically linked executables): each, started from the cell's files by
/// its interpreter, prints what it prints on Linux (`cells/expected.rs`, by
/// the differential check) and exits with 0. Their file pages are kept for
/// later cells, and once dropped every frame and heap allocation is back.
fn dynamically_linked_programs_behave_as_on_linux() {
    let packed = [crate::cell::dynamic_files(), crate::cell::distro_files()];
    assert!(packed[0].is_some(), "xtask packs the dynamic hello");
    let before = memory_balance();
    for &(path, argv, stdout) in expected::DYNAMIC {
        let held = |f: &&rhizo_fs::Fs| f.file_at(path.as_bytes()).is_ok();
        let Some(files) = packed.iter().flatten().find(held) else {
            let note = alloc::format!("({path} not packed: not run) ");
            arch::console::BootConsole.write_bytes(note.as_bytes());
            continue;
        };
        let mut host = Host::new();
        let id = spawn_dynamic(&mut host, files, path, argv);
        host.run(|_| false);
        assert_eq!(ended(&host, id), (Ending::Exit(0), stdout.into()), "{path}");
    }
    assert!(crate::cell::drop_images(), "file pages were kept");
    assert_eq!(memory_balance(), before, "nothing leaked");
}

/// A dynamically linked cell is plain data like any other (spec `cells`):
/// suspended once its interpreter has started it, resumed from the
/// snapshot's bytes, forked, and frozen as a template, every copy finishes
/// the program.
fn a_dynamically_linked_cell_suspends_resumes_and_forks() {
    let files = crate::cell::dynamic_files().expect("xtask packs the dynamic hello");
    let &(path, argv, stdout) = expected::DYNAMIC
        .iter()
        .find(|p| p.1 == ["hello", "wait"])
        .expect("hello wait");
    let rest = stdout.strip_prefix("ready\n").expect("ready first");
    let mut host = Host::new();
    let ready = |host: &mut Host| {
        let id = spawn_dynamic(host, &files, path, argv);
        while host.output(id) != Some(b"ready\n") {
            host.step();
        }
        id
    };
    let id = ready(&mut host);
    let bytes = host.suspend(id).expect("live").into_bytes();
    let snapshot = Snapshot::from_bytes(bytes, crate::arch::user::MACHINE).expect("valid");
    let resumed = host
        .resume(snapshot, Resume::Lazy, true, true)
        .expect("resume");
    let children = host.fork(resumed, 2).expect("fork");
    let template = ready(&mut host);
    assert!(host.freeze(template));
    let from_template: Vec<u32> = (0..2)
        .map(|_| {
            host.spawn_from_template(template, true, true)
                .expect("fork")
        })
        .collect();
    host.run(|_| false);
    for id in [resumed].into_iter().chain(children).chain(from_template) {
        assert_eq!(ended(&host, id), (Ending::Exit(0), rest.into()));
    }
}

// --- H14: threads ------------------------------------------------------------

/// Raw `clone` and futexes (`threads`) and glibc's pthreads, mutexes and
/// condition variables (`libc_threads`) behave as on Linux, and a threaded
/// cell returns every frame: thread stacks, arenas, all of it.
fn threads_behave_as_on_linux_and_return_memory() {
    assert_no_leak("threads");
    assert_no_leak("libc_threads");
}

/// A thread prints and calls `exit_group` while the first spins: the cell
/// ends with every thread, and all its memory comes back.
fn spinning_thread_does_not_starve_its_siblings() {
    fn round() {
        let mut host = Host::new();
        let id = spawn(&mut host, "threads", &[b"starve"]);
        host.run(|h| h.finished(id).is_some());
        assert_eq!(ended(&host, id), (Ending::Exit(0), "not starved\n".into()));
        assert_eq!(host.live(), 0);
    }
    round();
    let before = memory::free_bytes();
    round();
    assert_eq!(memory::free_bytes(), before, "every frame returned");
}

/// Scheduling steps until a `chatter` cell ends next to a cell of `n`
/// spinning threads.
fn steps_next_to_spinners(n: &[u8]) -> usize {
    let mut host = Host::new();
    let spinner = spawn(&mut host, "threads", &[b"spin", n]);
    let chatter = spawn(&mut host, "chatter", &[]);
    let mut steps = 0;
    while host.finished(chatter).is_none() {
        host.step();
        steps += 1;
    }
    assert!(host.kill(spinner));
    steps
}

fn thread_count_does_not_buy_cpu_time() {
    let one = steps_next_to_spinners(b"1");
    let four = steps_next_to_spinners(b"4");
    assert_eq!(four, one, "the cell, not the thread, gets the time");
}

fn cells_whose_threads_all_wait_use_no_cpu() {
    let mut host = Host::new();
    let parked = spawn(&mut host, "threads", &[b"park"]);
    let hello = spawn(&mut host, "hello", &[]);
    host.run(|_| false); // returns: only a cell that can never run is left
    assert_eq!(
        ended(&host, hello),
        (Ending::Exit(0), "hello from a cell\n".into())
    );
    assert_eq!(host.live(), 1, "the waiting cell is still live");
    assert!(host.finished(parked).is_none());
    // Its threads still wait after a suspend and resume, and it can be killed.
    let snapshot = host.suspend(parked).expect("live");
    let parked = host
        .resume(snapshot, Resume::Lazy, true, false)
        .expect("resume");
    host.run(|_| false);
    assert!(host.kill(parked));
    assert_eq!(ended(&host, parked).0, Ending::Killed);
}

fn expected_output(name: &str) -> &'static str {
    expected::PROGRAMS
        .iter()
        .find(|p| p.0 == name)
        .unwrap_or_else(|| panic!("{name} is not in cells/expected.rs"))
        .1
}

/// `libc_threads` suspended after every scheduling step and resumed from
/// the snapshot's bytes (threads waiting on condition variables included),
/// eagerly and lazily: its output is the uninterrupted one.
fn threaded_cell_survives_suspend_and_resume() {
    for mode in [Resume::Eager, Resume::Lazy] {
        // Progress for the harness: the run prints nothing else meanwhile.
        arch::console::BootConsole.write_bytes(if mode == Resume::Eager {
            b"(eager) "
        } else {
            b"(lazy) "
        });
        let mut host = Host::new();
        let mut id = spawn(&mut host, "libc_threads", &[]);
        let mut output = Vec::new();
        let mut resumes = 0;
        loop {
            host.step();
            if let Some(done) = host.finished(id) {
                assert_eq!(done.ending, Ending::Exit(0));
                output.extend_from_slice(&done.output);
                break;
            }
            let snapshot = host.suspend(id).expect("live");
            output.extend_from_slice(&host.finished(id).expect("suspended").output);
            let snapshot = Snapshot::from_bytes(snapshot.into_bytes(), crate::arch::user::MACHINE)
                .expect("valid");
            id = host.resume(snapshot, mode, true, false).expect("resume");
            resumes += 1;
        }
        assert!(resumes >= 20, "suspended {resumes} times");
        assert_eq!(
            alloc::string::String::from_utf8_lossy(&output),
            expected_output("libc_threads")
        );
    }
}

/// Spawns `libc_threads` and runs it until its workers wait for the go.
fn threaded_ready(host: &mut Host) -> u32 {
    let id = spawn(host, "libc_threads", &[]);
    while host.output(id) != Some(b"ready\n") {
        host.step();
    }
    id
}

/// Suspended at `ready` and resumed from its snapshot's bytes, `netstate`
/// finds its pipe's bytes, its listener and its connection (parked in the
/// kernel meanwhile); a copy of the same snapshot resumed after it finds
/// the connection reset (spec `networking`).
fn sockets_across_suspend_and_resume() {
    let mut host = Host::new();
    let id = netstate_ready(&mut host);
    let snapshot = host.suspend(id).expect("live");
    assert_eq!(
        crate::cell::open_sockets(),
        3,
        "a suspended cell's listener and connection's two ends, parked"
    );
    let bytes = snapshot.into_bytes();
    let copy = Snapshot::from_bytes(bytes.clone(), crate::arch::user::MACHINE).expect("valid");
    let snapshot = Snapshot::from_bytes(bytes, crate::arch::user::MACHINE).expect("valid");
    let id = host
        .resume(snapshot, Resume::Lazy, true, true)
        .expect("resume");
    host.run(|_| false);
    assert_eq!(
        ended(&host, id),
        (
            Ending::Exit(0),
            "pipe kept\nconnection open\nlistener ok\n".into()
        )
    );
    assert_eq!(crate::cell::open_sockets(), 0);
    let id = host
        .resume(copy, Resume::Eager, true, true)
        .expect("resume");
    host.run(|_| false);
    assert_eq!(
        ended(&host, id),
        (
            Ending::Exit(0),
            "pipe kept\nconnection reset\nlistener ok\n".into()
        )
    );
    assert_eq!(crate::cell::open_sockets(), 0);
}

/// A snapshot dropped without a resume closes its parked connections.
fn a_discarded_snapshot_closes_its_connections() {
    let mut host = Host::new();
    let id = netstate_ready(&mut host);
    let snapshot = host.suspend(id).expect("live");
    assert_eq!(crate::cell::open_sockets(), 3, "parked");
    crate::cell::close_parked(&snapshot);
    assert_eq!(crate::cell::open_sockets(), 0, "closed");
}

/// Runs `loopback serve PORT N` and `loopback ping PORT N` (with `addr`, or
/// against the server), suspending cell `moving` (0: server, 1: client)
/// after every scheduling step and resuming it at once, after `pause` when
/// given (a cell run meanwhile); returns the moving cell's whole output and
/// the number of resumes.
fn suspended_every_step(
    host: &mut Host,
    port: &[u8],
    n: &[u8],
    moving: usize,
    addr: Option<&[u8]>,
    pause: Option<&str>,
) -> (Vec<u8>, usize) {
    let mut ids = alloc::vec::Vec::new();
    if addr.is_none() {
        ids.push(spawn(host, "loopback", &[b"serve", port, n]));
    }
    let mut ping = alloc::vec![b"ping".as_slice(), port, n];
    ping.extend(addr);
    ids.push(spawn(host, "loopback", &ping));
    let mut output = Vec::new();
    let mut resumes = 0;
    loop {
        host.step();
        if let Some(done) = host.finished(ids[moving]) {
            assert_eq!(done.ending, Ending::Exit(0));
            output.extend_from_slice(&done.output);
            break;
        }
        let snapshot = host.suspend(ids[moving]).expect("live");
        output.extend_from_slice(&host.finished(ids[moving]).expect("suspended").output);
        if let Some(cell) = pause {
            let other = spawn(host, cell, &[]);
            host.run(|h| h.finished(other).is_some());
        }
        ids[moving] = host
            .resume(snapshot, Resume::Lazy, true, true)
            .expect("resume");
        resumes += 1;
    }
    host.run(|_| false);
    for (i, &id) in ids.iter().enumerate() {
        if i != moving {
            assert_eq!(ended(host, id).0, Ending::Exit(0));
        }
    }
    (output, resumes)
}

/// A loopback TCP connection between two cells keeps working while either
/// end is suspended and resumed after every scheduling step.
fn loopback_connection_across_suspend_of_one_end() {
    for (moving, port, said) in [(0, b"7304", "served\n"), (1, b"7305", "pinged\n")] {
        let mut host = Host::new();
        let (output, resumes) = suspended_every_step(&mut host, port, b"20", moving, None, None);
        assert!(resumes >= 20, "suspended {resumes} times");
        assert_eq!(alloc::string::String::from_utf8_lossy(&output), said);
        drop(host);
        assert_eq!(crate::cell::open_sockets(), 0);
    }
}

/// A connection to the host's echo server through the network card keeps
/// working while its cell is suspended after every step, with the card
/// polled meanwhile (a `sleeper` runs); not run without a card.
fn a_connection_to_the_host_survives_suspend() {
    let Some((ip, port)) = crate::wire::options().echo else {
        arch::console::BootConsole.write_bytes(b"(no network card and echo server: not run) ");
        return;
    };
    let mut host = Host::new();
    assert!(
        host.wire_address(10_000_000_000).is_some(),
        "the card gets an address"
    );
    let ip = alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    let port = alloc::format!("{port}");
    let (output, resumes) = suspended_every_step(
        &mut host,
        port.as_bytes(),
        b"10",
        0,
        Some(ip.as_bytes()),
        Some("sleeper"),
    );
    assert!(resumes >= 10, "suspended {resumes} times");
    assert_eq!(alloc::string::String::from_utf8_lossy(&output), "pinged\n");
    drop(host);
    assert_eq!(crate::cell::open_sockets(), 0);
}

/// A server suspended while it waits in `accept` waits there again after
/// its resume, and serves the client that comes.
/// Steps `host` until one cell waits on its descriptors (a server in
/// `accept`): a slice may end, preempted, before it gets there (under
/// emulation a tick can be due as the slice starts).
fn step_until_waiting(host: &mut Host) {
    for _ in 0..100 {
        if host.waiting() == 1 {
            return;
        }
        host.step();
    }
    assert_eq!(host.waiting(), 1, "the server waits in accept");
}

fn a_blocked_cell_survives_suspend() {
    let mut host = Host::new();
    let server = spawn(&mut host, "loopback", &[b"serve", b"7302", b"1"]);
    step_until_waiting(&mut host);
    assert_eq!(host.waiting(), 1);
    let snapshot = host.suspend(server).expect("live");
    let server = host
        .resume(snapshot, Resume::Eager, true, true)
        .expect("resume");
    let client = spawn(&mut host, "loopback", &[b"ping", b"7302", b"1"]);
    host.run(|_| false);
    assert_eq!(ended(&host, server), (Ending::Exit(0), "served\n".into()));
    assert_eq!(ended(&host, client), (Ending::Exit(0), "pinged\n".into()));
}

/// A server frozen while it waits in `accept`: two cells spawned from it
/// share its listener and each serves one of two clients.
fn forked_servers_share_their_listener() {
    let mut host = Host::new();
    let template = spawn(&mut host, "loopback", &[b"serve", b"7303", b"1"]);
    step_until_waiting(&mut host);
    assert!(host.freeze(template));
    let servers: Vec<u32> = (0..2)
        .map(|_| {
            host.spawn_from_template(template, true, true)
                .expect("fork")
        })
        .collect();
    let clients: Vec<u32> = (0..2)
        .map(|_| spawn(&mut host, "loopback", &[b"ping", b"7303", b"1"]))
        .collect();
    host.run(|_| false);
    for id in servers {
        assert_eq!(ended(&host, id), (Ending::Exit(0), "served\n".into()));
    }
    for id in clients {
        assert_eq!(ended(&host, id), (Ending::Exit(0), "pinged\n".into()));
    }
    drop(host);
    assert_eq!(
        crate::cell::open_sockets(),
        0,
        "the template's listener closed"
    );
}

/// Forked at `ready`, `netstate` and its child both read the pipe's bytes;
/// the child's connection is reset, the parent's is not, and both accept
/// on the listener they share.
fn pipes_and_sockets_of_a_fork() {
    let mut host = Host::new();
    let parent = netstate_ready(&mut host);
    let child = host.fork(parent, 1).expect("fork")[0];
    host.run(|_| false);
    assert_eq!(
        ended(&host, parent),
        (
            Ending::Exit(0),
            "ready\npipe kept\nconnection open\nlistener ok\n".into()
        )
    );
    assert_eq!(
        ended(&host, child),
        (
            Ending::Exit(0),
            "pipe kept\nconnection reset\nlistener ok\n".into()
        )
    );
    assert_eq!(crate::cell::open_sockets(), 0);
}

/// Frozen at `ready`, `netstate` keeps its connection as a template; a cell
/// spawned from it finds the connection reset and accepts on the listener
/// it shares with the template.
fn connections_of_a_template_reset_in_its_cells() {
    let mut host = Host::new();
    let template = netstate_ready(&mut host);
    assert!(host.freeze(template));
    let id = host
        .spawn_from_template(template, true, true)
        .expect("spawn");
    host.run(|_| false);
    assert_eq!(
        ended(&host, id),
        (
            Ending::Exit(0),
            "pipe kept\nconnection reset\nlistener ok\n".into()
        )
    );
    assert_eq!(crate::cell::open_sockets(), 3, "the template's");
    drop(host);
    assert_eq!(crate::cell::open_sockets(), 0);
}

/// A cell reaches a TCP echo server on the host through the network card
/// (`echo=` on the command line, given by `xtask` where the VMM has a
/// card); without one the test says it did not run.
fn a_cell_reaches_the_host() {
    let Some((ip, port)) = crate::wire::options().echo else {
        arch::console::BootConsole.write_bytes(b"(no network card and echo server: not run) ");
        return;
    };
    let mut host = Host::new();
    assert!(
        host.wire_address(10_000_000_000).is_some(),
        "the card gets an address"
    );
    let ip = alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    let port = alloc::format!("{port}");
    let id = spawn(
        &mut host,
        "loopback",
        &[b"ping", port.as_bytes(), b"20", ip.as_bytes()],
    );
    host.run(|_| false);
    assert_eq!(ended(&host, id), (Ending::Exit(0), "pinged\n".into()));
}

fn forks_of_a_threaded_cell_run_all_threads() {
    let after_ready = expected_output("libc_threads")
        .strip_prefix("ready\n")
        .expect("ready first");
    let mut host = Host::new();
    let parent = threaded_ready(&mut host);
    let children = host.fork(parent, 2).expect("fork");
    host.run(|_| false);
    assert_eq!(
        ended(&host, parent),
        (Ending::Exit(0), expected_output("libc_threads").into())
    );
    for child in children {
        assert_eq!(ended(&host, child), (Ending::Exit(0), after_ready.into()));
    }
}

fn template_of_a_threaded_program() {
    let after_ready = expected_output("libc_threads")
        .strip_prefix("ready\n")
        .expect("ready first");
    let mut host = Host::new();
    let template = threaded_ready(&mut host);
    assert!(host.freeze(template));
    let ids: Vec<u32> = (0..3)
        .map(|_| {
            host.spawn_from_template(template, true, true)
                .expect("spawn")
        })
        .collect();
    host.run(|_| false);
    for id in ids {
        assert_eq!(ended(&host, id), (Ending::Exit(0), after_ready.into()));
    }
}

/// The current stack pointer, roughly: the address of a local.
fn stack_address(_: u64) -> u64 {
    let local = 0u8;
    core::hint::black_box(&local) as *const u8 as u64
}

fn cpu_number(_: u64) -> u64 {
    arch::cpu::index() as u64
}

fn every_cpu_has_its_number_and_own_stack() {
    let mut stacks = Vec::new();
    stacks.push(stack_address(0));
    assert_eq!(arch::cpu::index(), 0, "tests run on the boot CPU");
    for cpu in 1..crate::smp::count() {
        crate::smp::post(cpu, cpu_number, 0);
        assert_eq!(crate::smp::join(cpu), cpu as u64);
        crate::smp::post(cpu, stack_address, 0);
        stacks.push(crate::smp::join(cpu));
    }
    for (cpu, &sp) in stacks.iter().enumerate() {
        let guard = crate::smp::stack_guard(cpu);
        assert!(
            sp > guard && sp - guard <= 0x1_1000,
            "CPU {cpu}: stack {sp:#x} is not above its guard {guard:#x}"
        );
        assert!(
            memory::translate(guard).is_none(),
            "CPU {cpu}: guard mapped"
        );
    }
}

/// Shared by the CPUs in `kernel_globals_are_safe_across_cpus`.
static COUNTER: crate::sync::Global<(u64, u64)> = crate::sync::Global::new((0, 0));

/// Increments both halves of [`COUNTER`] `rounds` times, allocating and
/// freeing kernel heap in between; returns the CPU's number.
fn hammer(rounds: u64) -> u64 {
    for i in 0..rounds {
        COUNTER.with(|(a, b)| {
            *a += 1;
            core::hint::black_box(&*a);
            *b += 1;
        });
        let boxed = Box::new([i; 16]);
        assert_eq!(core::hint::black_box(&boxed)[15], i);
    }
    arch::cpu::index() as u64
}

fn kernel_globals_are_safe_across_cpus() {
    const ROUNDS: u64 = 20_000;
    let cpus = crate::smp::count();
    let before = COUNTER.with(|c| *c);
    let heap_before = crate::heap::stats();
    for cpu in 1..cpus {
        crate::smp::post(cpu, hammer, ROUNDS);
    }
    assert_eq!(hammer(ROUNDS), 0);
    for cpu in 1..cpus {
        assert_eq!(crate::smp::join(cpu), cpu as u64);
    }
    let added = ROUNDS * cpus as u64;
    assert_eq!(COUNTER.with(|c| *c), (before.0 + added, before.1 + added));
    assert_eq!(
        crate::heap::stats().live,
        heap_before.live,
        "every allocation freed"
    );
}

/// Every CPU online, as a mask.
fn all_cpus() -> u32 {
    u32::MAX >> (32 - crate::smp::count())
}

/// Two CPU-bound cells per CPU, started together: they all finish, and
/// between them they ran on every CPU (spec `multi-core`, cells in
/// parallel).
fn cells_run_on_every_cpu() {
    let mut host = Host::new();
    let ids: Vec<u32> = (0..2 * crate::smp::count())
        .map(|_| spawn(&mut host, "spin", &[b"20"]))
        .collect();
    host.run(|_| false);
    let mut cpus = 0;
    for id in ids {
        assert_eq!(ended(&host, id).0, Ending::Exit(0));
        cpus |= host.finished(id).expect("ended").cpus;
    }
    assert_eq!(cpus, all_cpus(), "the cells used every CPU");
}

/// More cells spinning for ever than there are CPUs: a cell that prints
/// and exits still gets its turn (spec `scheduling`, preemption, on every
/// CPU).
fn busy_cpus_do_not_starve_a_cell() {
    let mut host = Host::new();
    let spinners: Vec<u32> = (0..crate::smp::count() + 1)
        .map(|_| spawn(&mut host, "spin", &[]))
        .collect();
    let hello = spawn(&mut host, "hello", &[]);
    host.run(|h| h.finished(hello).is_some());
    assert_eq!(
        ended(&host, hello),
        (Ending::Exit(0), "hello from a cell\n".into())
    );
    assert_eq!(host.live(), spinners.len(), "the spinners still run");
    for id in spinners {
        assert!(host.kill(id));
    }
}

/// Four copies each of programs that use memory, files, threads and time,
/// all at once on every CPU: each prints exactly what it prints alone, and
/// all memory comes back.
fn cells_in_parallel_stay_isolated() {
    let programs = ["counter", "filer", "libc_threads", "chatter", "threads"];
    let round = || {
        let mut host = Host::new();
        let mut ids = Vec::new();
        for _ in 0..4 {
            for name in programs {
                ids.push((name, spawn(&mut host, name, &[])));
            }
        }
        host.run(|_| false);
        for (name, id) in ids {
            assert_eq!(
                ended(&host, id),
                (Ending::Exit(0), expected_output(name).into()),
                "{name}"
            );
        }
        drop(host);
        crate::cell::drop_images(); // free memory with no image kept loaded
    };
    // The first round grows the kernel heap, which keeps the frames it grows
    // into (as `assert_no_leak` does), so the check does not depend on which
    // tests ran before this one.
    round();
    let before = memory::free_bytes();
    round();
    assert_eq!(memory::free_bytes(), before);
}

/// `counter` next to spinning cells, run on every CPU twenty slices (of
/// the boot CPU) at a time, then suspended and resumed from its snapshot's bytes, until it
/// ends: the host stops every CPU before it suspends, so the output is the
/// uninterrupted one (spec `multi-core`, a cell is stopped everywhere before
/// a snapshot).
fn suspend_between_parallel_runs_is_invisible() {
    let mut host = Host::new();
    let spinners: Vec<u32> = (0..crate::smp::count())
        .map(|_| spawn(&mut host, "spin", &[]))
        .collect();
    let mut id = spawn(&mut host, "counter", &[]);
    let mut output = Vec::new();
    loop {
        let mut slices = 0;
        host.run(|h| {
            slices += 1;
            slices > 20 || h.finished(id).is_some()
        });
        if let Some(done) = host.finished(id) {
            assert_eq!(done.ending, Ending::Exit(0));
            output.extend_from_slice(&done.output);
            break;
        }
        let snapshot = host.suspend(id).expect("the counter is live");
        output.extend_from_slice(&host.finished(id).expect("suspended").output);
        let bytes = snapshot.into_bytes();
        let snapshot = Snapshot::from_bytes(bytes, crate::arch::user::MACHINE).expect("valid");
        id = host
            .resume(snapshot, Resume::Lazy, true, false)
            .expect("resume");
    }
    assert_eq!(
        alloc::string::String::from_utf8_lossy(&output),
        expected_output("counter")
    );
    for id in spinners {
        assert!(host.kill(id));
    }
}

/// `parsum` with `threads` threads summing `millions` million rounds,
/// alone on every CPU: its output, and the most CPUs that ran its threads
/// at the same time.
fn parsum(threads: &[u8], millions: &[u8]) -> (alloc::string::String, u32) {
    let mut host = Host::new();
    let id = spawn(&mut host, "parsum", &[threads, millions]);
    host.run(|h| h.finished(id).is_some());
    let (ending, output) = ended(&host, id);
    assert_eq!(ending, Ending::Exit(0), "{output}");
    (output, host.finished(id).expect("ended").peak)
}

/// Four threads of one cell on four CPUs: the same sum as one thread (and
/// as Linux, `cells/expected.rs`), with threads of the cell running on
/// several CPUs at the same time (spec `threads`, threads of a cell in
/// parallel; `scheduling`, idle CPUs run threads of running cells). The
/// speed-up itself is measured by `cell.thread_scaling_pct`, not here: it
/// depends on the host's spare cores.
fn threads_of_a_cell_run_on_several_cpus_at_once() {
    run_expected("parsum");
    let (one, _) = parsum(b"1", b"24");
    let (four, peak) = parsum(b"4", b"24");
    assert_eq!(four, one, "the sum does not depend on the threads");
    assert_eq!(one, "sum 2868a16807fecd08\n");
    if crate::smp::count() >= 2 {
        assert!(peak >= 2, "its threads ran on {peak} CPU at a time");
    } else {
        assert_eq!(peak, 1, "one CPU: one thread at a time");
    }
}

/// A thread keeps reading a page that another unmaps on another CPU: once
/// the unmap has returned, the reader's next read faults; it never reads
/// the page after it saw the flag raised after the unmap (spec
/// `multi-core`, TLB coherence; on x86_64 a shootdown). Several rounds, the
/// race being the point; after the first (which caches the executable),
/// every frame comes back.
fn unmapped_page_is_gone_on_every_cpu() {
    run_expected("shootdown");
    let before = memory::free_bytes();
    for _ in 0..4 {
        run_expected("shootdown");
    }
    assert_eq!(memory::free_bytes(), before, "every frame returned");
}

/// `parsum` computing with 4 threads on every CPU, stopped after each slice
/// of the boot CPU, then suspended and resumed from its snapshot's bytes,
/// or once forked into two children, until it ends: the resumed cell and
/// both children print the uninterrupted sum (spec `multi-core`, cells
/// stopped before host operations). Every cell ends; the frames of cells
/// that ran on several CPUs are checked by `unmapped_page_is_gone_on_every_cpu`
/// and `cells_in_parallel_stay_isolated`.
fn a_cell_on_several_cpus_survives_suspend_and_fork() {
    const SUM: &str = "sum c75f7d3df19f452d\n";
    let mut host = Host::new();
    let mut id = spawn(&mut host, "parsum", &[b"4", b"96"]);
    let mut children = Vec::new();
    let mut output = Vec::new();
    let mut round = 0;
    loop {
        // Asked before the run starts, then after each slice.
        let mut started = false;
        host.run(|_| core::mem::replace(&mut started, true));
        if host.finished(id).is_some() {
            break;
        }
        round += 1;
        if round == 1 {
            children = host.fork(id, 2).expect("fork");
            continue;
        }
        let snapshot = host.suspend(id).expect("live");
        output.extend_from_slice(&host.finished(id).expect("suspended").output);
        let snapshot =
            Snapshot::from_bytes(snapshot.into_bytes(), crate::arch::user::MACHINE).expect("valid");
        id = host
            .resume(snapshot, Resume::Lazy, true, false)
            .expect("resume");
    }
    assert!(round >= 2, "stopped only {round} times");
    host.run(|_| false);
    let (ending, last) = ended(&host, id);
    assert_eq!(ending, Ending::Exit(0));
    output.extend_from_slice(last.as_bytes());
    assert_eq!(alloc::string::String::from_utf8_lossy(&output), SUM);
    for id in children {
        assert_eq!(
            ended(&host, id),
            (Ending::Exit(0), SUM.into()),
            "child {id}"
        );
    }
    assert_eq!(host.live(), 0);
}

/// Where the CPU or hypervisor has performance counters, they count cycles
/// (and retired instructions where counted) in kernel code; without them
/// there is nothing to read (spec `profiling`).
fn performance_counters_count_where_present() {
    let Some(_) = arch::pmu::init() else {
        assert!(arch::pmu::read().is_none());
        return;
    };
    let before = arch::pmu::read().expect("counters on");
    let mut x = 0u64;
    for i in 0..10_000u64 {
        x = core::hint::black_box(x.wrapping_add(i));
    }
    let counted = arch::pmu::read().expect("counters on").since(before);
    assert!(counted.cycles > 0, "cycles did not advance");
    if let Some(instructions) = counted.instructions {
        assert!(instructions >= 10_000, "{instructions} instructions");
    }
}

/// With every part accounted, a cell making 100 `getpid` calls shows up as
/// cell time, system calls (100 of them `getpid`), spawn, teardown, frames
/// and copies, and the parts add up to no more than the time it took on
/// every CPU
/// (spec `profiling`).
fn cost_accounting_adds_up_over_a_cell() {
    use rhizo_prof::Part;
    let t0 = arch::timer::Counter.cycles();
    crate::cost::start(crate::cost::ALL);
    let (ending, _) = crate::cell::run(
        crate::cell::test_image("yielder"),
        &[b"yielder", b"getpid", b"100"],
    )
    .expect("spawn yielder");
    crate::cost::stop(|account, calls| {
        let elapsed = arch::timer::Counter.cycles() - t0;
        assert_eq!(ending, Ending::Exit(0));
        // Every CPU accounts its own time.
        let cpus = crate::smp::count() as u64;
        assert!(account.total() <= cpus * elapsed, "more time than elapsed");
        assert!(account.count(Part::Syscall) >= 101, "100 getpid and exit");
        assert!(account.count(Part::Cell) >= account.count(Part::Syscall));
        for part in [
            Part::Cell,
            Part::Syscall,
            Part::Spawn,
            Part::Teardown,
            Part::Frames,
            Part::Copy,
        ] {
            assert!(account.time(part) > 0, "no time in {}", part.name());
        }
        let (top, n) = calls.top::<8>();
        let getpid = top[..n]
            .iter()
            .find(|c| {
                alloc::format!("{:?}", arch::user::ABI.call_name(c.0).expect("known")) == "Getpid"
            })
            .expect("getpid recorded");
        assert_eq!(getpid.1, 100);
    });
}

/// Sampling (spec `profiling`): while a cell spins, every sample taken is
/// that cell's, at an address of its code, with the PMU's overflow
/// interrupt or, without one, the timer.
fn samples_show_where_a_cell_spins() {
    let mut host = Host::new();
    let id = spawn(&mut host, "spin", &[b"20"]);
    let (sampler, _) = crate::cost::start_sampling(4096);
    host.run(|h| h.finished(id).is_some());
    let samples = crate::cost::stop_sampling(|s| s.sorted().to_vec());
    assert_eq!(ended(&host, id).0, Ending::Exit(0));
    assert!(!samples.is_empty(), "no sample ({sampler})");
    for s in &samples {
        assert_eq!(s.cell, id, "{sampler}");
        assert!(s.pc < 0x0000_8000_0000_0000, "{:#x} ({sampler})", s.pc);
    }
}
