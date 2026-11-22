//! Suspend and resume (spec `snapshots`).

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(suspend_resume_after_every_step_is_invisible_eager, 174),
    ktest_case!(suspend_resume_after_every_step_is_invisible_lazy),
    ktest_case!(suspend_between_parallel_runs_is_invisible),
    ktest_case!(suspend_returns_all_memory),
    ktest_case!(lazy_resume_restores_only_touched_pages),
    ktest_case!(a_fork_short_of_frames_loses_no_page),
    ktest_case!(suspended_cells_hold_no_kernel_resources),
    ktest_case!(a_suspended_fork_refers_to_its_template),
    ktest_case!(snapshot_of_another_architecture_is_rejected),
];

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
        let bytes = snapshot.export(&DirectMap, crate::arch::mem::crc32c);
        let snapshot =
            Snapshot::from_bytes(bytes, crate::arch::user::MACHINE, crate::arch::mem::crc32c)
                .expect("valid");
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
    let before = free_or_in_heap();
    let snapshot = cycle();
    assert!(snapshot.heap_bytes() > 8 << 20, "8 MiB of pages captured");
    drop(snapshot);
    assert_eq!(
        free_or_in_heap(),
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

/// A cell resumed lazily and forked while no frame is free: the fork is
/// refused and the cell keeps every page it has not restored yet, so it
/// reads back what it wrote once frames are free again. Freezing it is
/// refused too. Pages a fork failed to restore were dropped, and read as
/// zeros (a Node.js cell resumed from bytes jumped to address 0).
fn a_fork_short_of_frames_loses_no_page() {
    let before = memory_balance();
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
    // The frames taken are listed in themselves: the heap stays as it is.
    let mut taken = u64::MAX;
    while let Some(frame) = memory::alloc_for_cell(1) {
        DirectMap.write_u64(frame, taken);
        taken = frame;
    }
    let forked = host.fork(id, 1);
    let frozen = host.freeze(id);
    while taken != u64::MAX {
        let next = DirectMap.read_u64(taken);
        memory::with_frames(|f| f.free_contiguous(taken, 1));
        taken = next;
    }
    assert_eq!(forked, None, "no copy without its pages");
    assert!(!frozen, "no template without its pages");
    let left = host.pending_pages(id).expect("still live");
    assert!(left > 0 && left <= pages, "{left} of {pages} pages pending");
    host.run(|_| false);
    assert_eq!(ended(&host, id), (Ending::Exit(0), "checked\n".into()));
    drop(host);
    assert_eq!(memory_balance(), before, "every frame back");
}

/// A cell forked from a template and suspended before it writes refers to
/// the template's pages instead of copying them (spec `snapshots`, change
/// `add-shared-snapshots`); resumed, it writes its own copies, and the
/// template's pages stay as they were.
fn a_suspended_fork_refers_to_its_template() {
    let mut host = Host::new();
    let template = spawn(&mut host, "ballast", &[b"write", b"2"]);
    while host.output(template) != Some(b"filled\n") {
        host.step();
    }
    assert!(host.freeze(template));
    let checked = (Ending::Exit(0), "checked\n".into());
    for mode in [Resume::Lazy, Resume::Eager] {
        let id = host
            .spawn_from_template(template, true, false)
            .expect("fork");
        let snapshot = host.suspend(id).expect("live");
        let (heap, shared) = (snapshot.heap_bytes(), snapshot.shared_bytes());
        assert!(shared >= 2 << 20, "{shared} bytes shared");
        assert!(heap < 64 << 10, "{heap} bytes in the heap");
        let id = host.resume(snapshot, mode, true, false).expect("resume");
        host.run(|h| h.finished(id).is_some());
        assert_eq!(ended(&host, id), checked, "{mode:?}");
    }
    let id = host
        .spawn_from_template(template, true, false)
        .expect("fork");
    host.run(|h| h.finished(id).is_some());
    assert_eq!(ended(&host, id), checked, "the template is unchanged");
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
    let mut bytes = host
        .suspend(id)
        .expect("live")
        .export(&DirectMap, crate::arch::mem::crc32c);
    let other: u16 = if crate::arch::user::MACHINE as u16 == 183 {
        62
    } else {
        183
    };
    bytes[12..14].copy_from_slice(&other.to_le_bytes());
    assert!(matches!(
        Snapshot::from_bytes(bytes, crate::arch::user::MACHINE, crate::arch::mem::crc32c),
        Err(FormatError::Machine { .. })
    ));
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
        let bytes = snapshot.export(&DirectMap, crate::arch::mem::crc32c);
        let snapshot =
            Snapshot::from_bytes(bytes, crate::arch::user::MACHINE, crate::arch::mem::crc32c)
                .expect("valid");
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
