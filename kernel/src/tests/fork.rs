//! Fork and templates (spec `fork`).

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(forked_cells_see_only_their_own_writes),
    ktest_case!(children_outlive_their_parent_and_frames_return),
    ktest_case!(forks_of_forks),
    ktest_case!(fork_allocates_only_a_root),
    ktest_case!(spawn_from_template_runs_like_the_program),
    ktest_case!(wipeonfork_reads_zero_in_copies),
    ktest_case!(generation_changes_only_in_copies),
    ktest_case!(copy_pid_differs),
    ktest_case!(loads_of_a_copied_template_see_new_generations),
    ktest_case!(tainted_template_not_published),
];

/// Spawns `forkme` and runs it until it is ready to be forked.
pub(super) fn forkme_ready(host: &mut Host) -> u32 {
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
    let before = free_or_in_heap();
    round();
    assert_eq!(
        free_or_in_heap(),
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

/// What `copies` printed at its end (`page P generation G0 G1 pid W`) in
/// the source of a fork, then in each kind of copy (design D22 of
/// `add-cell-hierarchy`): the fork's child, a cell from a template of it,
/// an eager and a lazy resume, and a load of its snapshot's bytes.
fn copies_report() -> (alloc::string::String, Vec<alloc::string::String>) {
    let ready = |host: &mut Host| {
        let id = spawn(host, "copies", &[]);
        host.run_to_pause(id, b"ready", Pause::Waits)
            .unwrap_or_else(|why| panic!("{why}: {:?}", ended(host, id)));
        id
    };
    let mut host = Host::new();
    let source = ready(&mut host);
    let mut copies = host.fork(source, 1).expect("fork");
    let template = ready(&mut host);
    assert!(host.freeze(template));
    copies.push(
        host.spawn_from_template(template, true, false)
            .expect("copy"),
    );
    for mode in [Resume::Eager, Resume::Lazy] {
        let id = ready(&mut host);
        let snapshot = host.suspend(id).expect("live");
        copies.push(host.resume(snapshot, mode, true, false).expect("resume"));
    }
    let id = ready(&mut host);
    let crc = crate::arch::mem::crc32c;
    let bytes = host.suspend(id).expect("live").export(&DirectMap, crc);
    let snapshot = Snapshot::from_bytes(bytes, crate::arch::user::MACHINE, crc).expect("valid");
    copies.push(
        host.resume(snapshot, Resume::Eager, true, false)
            .expect("load"),
    );
    host.run(|_| false);
    let last = |id| {
        let (ending, out) = ended(&host, id);
        assert_eq!(ending, Ending::Exit(0), "{out}");
        out.lines().last().unwrap_or_default().into()
    };
    (last(source), copies.into_iter().map(last).collect())
}

/// The words of `copies`' report after `key`, up to the next word that is
/// not a number or `same`/`changed`.
fn reported(line: &str, key: &str) -> alloc::string::String {
    let mut words = line.split(' ').skip_while(|&w| w != key).skip(1);
    let first = words.next().unwrap_or_default();
    match words.next() {
        Some(w) if w.bytes().all(|b| b.is_ascii_digit()) => alloc::format!("{first} {w}"),
        _ => first.into(),
    }
}

/// A page marked `MADV_WIPEONFORK` in which the cell wrote 1 reads 0 in
/// its fork, in a copy of its template, after a resume and after a load
/// of its snapshot; the source of the fork still reads 1 (spec
/// `linux-abi`, design D22 of `add-cell-hierarchy`).
fn wipeonfork_reads_zero_in_copies() {
    let (source, copies) = copies_report();
    assert_eq!(reported(&source, "page"), "1", "{source}");
    for copy in copies {
        assert_eq!(reported(&copy, "page"), "0", "{copy}");
    }
}

/// `/run/rhizo/generation` keeps its value in the source of a fork, and
/// reads another one in every copy: one more in a copy made in the
/// kernel, a value drawn at random in a load of bytes (design D22 of
/// `add-cell-hierarchy`).
fn generation_changes_only_in_copies() {
    let (source, copies) = copies_report();
    assert_eq!(reported(&source, "generation"), "0 0", "{source}");
    let (loaded, made_here) = copies.split_last().expect("copies");
    for copy in made_here {
        assert_eq!(reported(copy, "generation"), "0 1", "{copy}");
    }
    let generation = reported(loaded, "generation");
    assert!(
        generation.starts_with("0 ") && generation != "0 0",
        "{loaded}"
    );
}

/// A copy's `getpid` is never its source's (design D22 of
/// `add-cell-hierarchy`).
fn copy_pid_differs() {
    let (source, copies) = copies_report();
    assert_eq!(reported(&source, "pid"), "same", "{source}");
    for copy in copies {
        assert_eq!(reported(&copy, "pid"), "changed", "{copy}");
    }
}

/// A template of a cell that connected to the host's echo server, or of
/// its fork, may be published only on the host's word that it holds no
/// secret; one of a cell that never reached the network card may be
/// (design D22 of `add-cell-hierarchy`). Not run without a card.
fn tainted_template_not_published() {
    let mut host = Host::new();
    let quiet = spawn(&mut host, "copies", &[]);
    host.run_to_pause(quiet, b"ready", Pause::Waits)
        .unwrap_or_else(|why| panic!("{why}"));
    assert!(host.freeze(quiet));
    assert_eq!(host.may_publish(quiet, false), Some(true));
    assert_eq!(host.may_publish(quiet + 1000, true), None, "no template");
    let Some((ip, port)) = crate::wire::options().echo else {
        arch::console::BootConsole.write_bytes(b"(no network card and echo server: not run) ");
        return;
    };
    assert!(
        host.wire_address(10_000_000_000).is_some(),
        "the card gets an address"
    );
    let ip = alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    let port = alloc::format!("{port}");
    let id = spawn(
        &mut host,
        "copies",
        &[b"connect", port.as_bytes(), ip.as_bytes()],
    );
    while host.output(id) != Some(b"connected\n") {
        assert!(host.finished(id).is_none(), "{:?}", ended(&host, id));
        host.step();
    }
    let child = host.fork(id, 1).expect("fork")[0];
    // Records of earlier tests out of the way: nothing empties the ring
    // in a test build.
    while crate::audit::take().is_some() {}
    for template in [id, child] {
        assert!(host.freeze(template));
        assert_eq!(host.may_publish(template, false), Some(false), "refused");
        assert_eq!(host.may_publish(template, true), Some(true), "attested");
        // Both audited (design D18), naming the template and its domain.
        let domain = host.tree().root_of(template).expect("in the tree");
        for kind in [crate::audit::Kind::Publish, crate::audit::Kind::Attested] {
            let r = crate::audit::take().expect("an audit record");
            assert_eq!(
                (r.kind, r.cell, r.domain),
                (kind, template, domain),
                "{r:?}"
            );
        }
    }
}

/// Two loads of the bytes of a template that was itself a copy (it read
/// generation 1 before it was frozen) each read a generation that is not
/// 1 and not the other's, so each reseeds and draws its own numbers
/// (design D22 of `add-cell-hierarchy`: the bytes hold the template's
/// generation, and each load draws its own).
fn loads_of_a_copied_template_see_new_generations() {
    let mut host = Host::new();
    let first = spawn(&mut host, "copies", &[b"wait"]);
    host.run_to_pause(first, b"start", Pause::Waits)
        .unwrap_or_else(|why| panic!("{why}"));
    assert!(host.freeze(first));
    let copy = host.spawn_from_template(first, true, false).expect("copy");
    host.run_to_pause(copy, b"ready", Pause::Waits)
        .unwrap_or_else(|why| panic!("{why}: {:?}", ended(&host, copy)));
    assert!(host.freeze(copy));
    let crc = crate::arch::mem::crc32c;
    let bytes = host
        .template_snapshot(copy)
        .expect("snapshot")
        .export(&DirectMap, crc);
    let loads: Vec<u32> = (0..2)
        .map(|_| {
            let snapshot = Snapshot::from_bytes(bytes.clone(), crate::arch::user::MACHINE, crc)
                .expect("valid");
            host.resume(snapshot, Resume::Eager, true, false)
                .expect("load")
        })
        .collect();
    host.run(|_| false);
    let seen: Vec<alloc::string::String> = loads
        .iter()
        .map(|&id| {
            let (ending, out) = ended(&host, id);
            assert_eq!(ending, Ending::Exit(0), "{out}");
            let generation = reported(&out, "generation");
            let (before, after) = generation.split_once(' ').expect("two values");
            assert_eq!(before, "1", "{out}");
            assert!(after != "1" && after != "0", "{out}");
            after.into()
        })
        .collect();
    assert_ne!(seen[0], seen[1], "two loads alike");
}
