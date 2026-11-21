//! Cells: spawning, running, faults and isolation between cells.

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
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
];

/// Runs test cell `name` and checks its output and ending against
/// `cells/expected.rs`; returns everything it printed.
pub(crate) fn run_expected(name: &str) -> alloc::string::String {
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
pub(super) fn assert_no_leak(name: &str) -> alloc::string::String {
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

pub(super) fn spawn(host: &mut Host, name: &str, args: &[&[u8]]) -> u32 {
    let mut argv = alloc::vec![name.as_bytes()];
    argv.extend_from_slice(args);
    host.spawn(crate::cell::test_image(name), &argv, true, true)
        .unwrap_or_else(|e| panic!("cannot spawn {name}: {e:?}"))
}

pub(super) fn ended(host: &Host, id: u32) -> (Ending, alloc::string::String) {
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
    let before = free_or_in_heap();
    for _ in 0..150 {
        pair();
    }
    assert_eq!(free_or_in_heap(), before);
}
