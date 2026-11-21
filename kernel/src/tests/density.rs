//! Memory accounting and density (H9), and waking sleeping cells.

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(memory_pressure_fails_cells_not_the_kernel, 48),
    ktest_case!(a_thousand_idle_cells, 37),
    ktest_case!(host_operations_find_cells_among_many_sleepers, 175),
    ktest_case!(memory_full_of_cells_is_refused_cleanly),
    ktest_case!(a_woken_cell_runs_at_once),
    ktest_case!(a_wake_before_the_sleep_is_kept),
    ktest_case!(a_wait_on_descriptors_keeps_no_time),
    ktest_case!(a_suspended_wait_on_descriptors_keeps_its_time_left, 40),
    ktest_case!(a_template_travels_as_bytes),
];

/// Memory that is not leaked: frames free in the allocator plus frames the
/// kernel heap holds (it keeps some small-object pages for reuse), and
/// the heap's live allocations; measured with no image kept loaded (spec
/// `cells`, image cache: those frames are the kernel's until dropped) and
/// no connection through the card still closing ([`settle_wire`]).
pub(super) fn memory_balance() -> (u64, usize) {
    settle_wire();
    crate::cell::drop_images();
    (free_or_in_heap(), crate::heap::stats().live)
}

/// Waits until every connection through the network card has finished
/// closing (`smoltcp` keeps one until its peer acknowledges the close),
/// polling the card for at most 10 seconds: what an earlier test left
/// closing is not counted against a later one (change
/// `shard-kernel-tests`), and a connection that never closes fails.
pub(super) fn settle_wire() {
    let mut host = Host::new();
    let end = crate::cell::now_ns() + 10_000_000_000;
    while crate::cell::wire_busy() && crate::cell::now_ns() < end {
        host.wire_address(0);
    }
    assert!(!crate::cell::wire_busy(), "the card's connections closed");
}

/// Frames free in the allocator plus the frames the kernel heap holds: the
/// same whenever the heap gives its free pages back (change
/// `add-heap-trim`), which depends on what ran before.
pub(super) fn free_or_in_heap() -> u64 {
    memory::free_bytes() + crate::heap::stats().pages as u64 * FRAME_SIZE
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
        // Its copies, each a root, share it only once published (design
        // D21 of `add-cell-hierarchy`), as the host's templates are.
        assert_eq!(host.publish(template, false), Ok(()));
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

/// Host operations find a cell among hundreds of sleepers by its
/// identifier (design D2 of `heap-caches-cell-index`), between runs and
/// from a run's stop condition while the other CPUs run.
fn host_operations_find_cells_among_many_sleepers() {
    const N: usize = 300;
    let mut host = Host::new();
    let template = spawn(&mut host, "idle", &[]);
    while host.output(template) != Some(b"idle\n") {
        host.step();
    }
    assert!(host.freeze(template));
    let mut ids: Vec<u32> = (0..N)
        .map(|_| {
            host.spawn_from_template(template, false, false)
                .expect("fork")
        })
        .collect();
    // Each runs into its sleep (once all sleep, a step would wait an hour).
    for &id in &ids {
        while !host.asleep(id) {
            host.step();
        }
    }
    // Between runs: suspended, resumed and killed, every seventh from the
    // last.
    for i in (0..N).rev().step_by(7) {
        let id = ids.swap_remove(i);
        let snapshot = host.suspend(id).expect("suspend a sleeper");
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Suspended));
        assert!(!host.asleep(id) && !host.kill(id), "{id} is gone");
        let back = host
            .resume(snapshot, Resume::Lazy, false, false)
            .expect("resume");
        assert!(host.kill(back));
    }
    assert_eq!(host.live(), ids.len());
    // From the stop condition: each forked, its child and itself killed.
    host.look_every(Some(1_000_000));
    let mut left = ids.clone();
    host.run(|h| {
        let Some(id) = left.pop() else {
            return true;
        };
        let child = h.fork(id, 1).expect("fork a sleeper")[0];
        assert!(h.kill(child) && h.kill(id));
        assert!(!h.kill(id), "{id} is gone");
        false
    });
    assert_eq!(host.live(), 0);
    for id in ids {
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Killed));
    }
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
    // The heap gave back the pages of the cells' kernel records (change
    // `add-heap-trim`): it keeps at most what its next trim waits for.
    let spike = o.heap_peak_bytes;
    assert!(
        spike > 2 * TRIM_AT as u64,
        "a spike of {spike} bytes: {o:?}"
    );
    assert!(o.heap_kept_bytes <= 2 * TRIM_AT as u64, "{o:?}");
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

/// A wait on descriptors with a timeout (an hour in `epoll_pwait`, as
/// libuv waits for its next timer) is timed out in a cell spawned from a
/// template frozen during it, and by a wake from the host: the call, made
/// again, returns at once instead of waiting its whole timeout again
/// (design D1 of `fast-node-template`; every fork of the Node.js template
/// waited its 50 ms pause again).
fn a_wait_on_descriptors_keeps_no_time() {
    let mut host = Host::new();
    let waiting = |host: &mut Host| {
        let id = spawn(host, "sleeper", &[b"3600000", b"epoll"]);
        while !(host.output(id) == Some(b"ready\n") && host.waits(id)) {
            host.step();
        }
        id
    };
    let template = waiting(&mut host);
    assert!(host.freeze(template));
    let child = host
        .spawn_from_template(template, true, false)
        .expect("fork");
    let woken = waiting(&mut host);
    assert!(host.wake(woken));
    // A slice each: the call made again, `timed out`, the exit.
    for _ in 0..4 {
        if host.finished(child).is_none() || host.finished(woken).is_none() {
            host.step();
        }
    }
    // The fork's output starts after the template's `ready`.
    assert_eq!(ended(&host, child), (Ending::Exit(0), "timed out\n".into()));
    assert_eq!(
        ended(&host, woken),
        (Ending::Exit(0), "ready\ntimed out\n".into())
    );
}

/// A cell suspended 1.5 s into a 2 s wait in `epoll_pwait`, then resumed
/// from its exported snapshot, times out about 0.5 s after the resume, not
/// 2 s (design D5 of `snapshots-on-disk`: its snapshot keeps the call's
/// time left).
fn a_suspended_wait_on_descriptors_keeps_its_time_left() {
    const WAIT_NS: u64 = 2_000_000_000;
    const BEFORE_NS: u64 = 1_500_000_000;
    let mut host = Host::new();
    let id = spawn(&mut host, "sleeper", &[b"2000", b"epoll"]);
    while !(host.output(id) == Some(b"ready\n") && host.waits(id)) {
        host.step();
    }
    let waited = crate::cell::now_ns();
    while crate::cell::now_ns() - waited < BEFORE_NS {
        crate::cell::timed_wait(BEFORE_NS / 10);
    }
    let bytes = host
        .suspend(id)
        .expect("live")
        .export(&DirectMap, crate::arch::mem::crc32c);
    let snapshot =
        Snapshot::from_bytes(bytes, crate::arch::user::MACHINE, crate::arch::mem::crc32c)
            .expect("valid");
    let left: Vec<u64> = snapshot.deadlines(0).iter().map(|d| d.1).collect();
    assert!(
        matches!(left[..], [ns] if ns > 0 && ns <= WAIT_NS - BEFORE_NS),
        "time left {left:?}"
    );
    let t0 = crate::cell::now_ns();
    let id = host
        .resume(snapshot, Resume::Lazy, true, false)
        .expect("resume");
    host.run(|h| h.finished(id).is_some());
    let took = crate::cell::now_ns() - t0;
    assert_eq!(ended(&host, id), (Ending::Exit(0), "timed out\n".into()));
    assert!(took < BEFORE_NS, "timed out {took} ns after the resume");
}

/// A template saved as bytes and loaded back (design D3 of
/// `snapshots-on-disk`) is a template: never run, its copies start where
/// it was frozen and keep no time (an hour in `epoll_pwait` times out at
/// once), and every frame comes back once both templates are dropped.
fn a_template_travels_as_bytes() {
    let before = memory_balance();
    let mut host = Host::new();
    let id = spawn(&mut host, "sleeper", &[b"3600000", b"epoll"]);
    while !(host.output(id) == Some(b"ready\n") && host.waits(id)) {
        host.step();
    }
    assert!(host.freeze(id));
    let bytes = host
        .template_snapshot(id)
        .expect("a template")
        .export(&DirectMap, crate::arch::mem::crc32c);
    let snapshot =
        Snapshot::from_bytes(bytes, crate::arch::user::MACHINE, crate::arch::mem::crc32c)
            .expect("valid");
    let loaded = host.load_template(snapshot).expect("fits");
    assert!(host.drop_template(id));
    assert!(host.template_snapshot(id).is_none(), "dropped");
    let copies: Vec<u32> = (0..2)
        .map(|_| host.spawn_from_template(loaded, true, false).expect("copy"))
        .collect();
    for _ in 0..6 {
        if copies.iter().any(|&c| host.finished(c).is_none()) {
            host.step();
        }
    }
    for c in copies {
        assert_eq!(ended(&host, c), (Ending::Exit(0), "timed out\n".into()));
    }
    assert!(host.drop_template(loaded));
    drop(host);
    assert_eq!(memory_balance(), before, "every frame back");
}
