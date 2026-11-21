//! Isolation under attack (H13, spec `isolation`).

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(hostile_pointers_get_efault),
    ktest_case!(cell_cannot_write_kernel_memory),
    ktest_case!(syscall_fuzzer_cannot_hurt_the_kernel),
    ktest_case!(syscall_storm_does_not_starve_others),
    ktest_case!(memory_hog_is_refused_and_others_run),
    ktest_case!(fresh_memory_never_holds_another_cells_data, 32),
    ktest_case!(names_cannot_fill_kernel_memory, 91),
    ktest_case!(shared_read_only_pages_stay_shared_after_mprotect),
];

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
