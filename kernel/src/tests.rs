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
            ktest_case!(suspend_resume_after_every_step_is_invisible_eager),
            ktest_case!(suspend_resume_after_every_step_is_invisible_lazy),
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

/// In self-check builds the regular suite is compiled but not run (it runs
/// in the `ktest` build): referenced here so it keeps compiling everywhere.
#[cfg(any(
    feature = "ktest-should-panic",
    feature = "ktest-hang",
    feature = "ktest-page-fault",
    feature = "ktest-stack-overflow",
    feature = "ktest-invalid-opcode",
    feature = "ktest-oom",
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

/// Runs `name` once to warm the kernel heap (which keeps the frames it
/// grows into), then checks that running it again leaves free memory
/// unchanged: every frame of the cell came back.
fn assert_no_leak(name: &str) -> alloc::string::String {
    run_expected(name);
    let before = memory::free_bytes();
    let text = run_expected(name);
    assert_eq!(
        memory::free_bytes(),
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
    let before = memory::free_bytes();
    assert_eq!(
        crate::cell::run(b"not an elf", &[]).map(|r| r.0),
        Err(ElfError::NotElf)
    );
    assert!(matches!(
        crate::cell::run(&other, &[]).map(|r| r.0),
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
    let sleeper = spawn(&mut host, "sleeper", &[]);
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
