//! Content sharing bounded by the domain (design D21 of
//! `add-cell-hierarchy`): file pages kept per domain, public objects, what
//! copies share, and cells leaving their domain.

use super::*;
use crate::cell::{Refused, kept_pages_of, test_image};
use rhizo_abi::PUBLIC;
use rhizo_cap::{Actor, HOST, Limits, Spec};
use rhizo_cell::MMAP_BASE;
use rhizo_cell::quota::Quota;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(same_file_in_two_domains),
    ktest_case!(public_library_always_hits),
    ktest_case!(file_pages_kept_per_domain),
    ktest_case!(publication_decides_what_copies_share),
    ktest_case!(template_dropped_under_two_domains),
    ktest_case!(promotion_unshares),
    ktest_case!(a_moved_snapshot_unshares),
    ktest_case!(a_template_snapshot_resumes_lazily_elsewhere),
];

const MIB: u64 = 1 << 20;
/// Pages a cell may copy, or the kernel take, without that being the
/// copies of a write over one MiB (256 pages).
const NOISE: u64 = 64 * FRAME_SIZE;

/// A `scoped` cell mapping the first `pages` pages of `path` in `files`, a
/// root of its own domain.
fn mapper(host: &mut Host, files: &rhizo_fs::Fs, path: &str, pages: &str) -> u32 {
    let argv: [&[u8]; 4] = [b"scoped", b"map", path.as_bytes(), pages.as_bytes()];
    host.spawn_with_files(test_image("scoped"), &argv, files.clone(), true, true)
        .expect("spawn scoped")
}

/// Line `n` (from 1) of what cell `id` printed, once it has.
fn line(host: &mut Host, id: u32, n: usize) -> alloc::string::String {
    loop {
        let out = alloc::string::String::from_utf8_lossy(host.output(id).unwrap_or_default());
        // Only whole lines: the cell prints one in several writes.
        if let Some(line) = out
            .split_inclusive('\n')
            .nth(n - 1)
            .and_then(|l| l.strip_suffix('\n'))
        {
            return line.into();
        }
        assert!(host.finished(id).is_none(), "{:?}", ended(host, id));
        host.step();
    }
}

/// Where `mapper` cell `id` mapped its file in round `round` (from 1).
fn mapped_at(host: &mut Host, id: u32, round: usize) -> u64 {
    let line = line(host, id, round);
    let hex = line.strip_prefix("at 0x").expect("an address");
    u64::from_str_radix(hex, 16).expect("hex")
}

/// Wakes `mapper` cell `id` for its next round and returns where it mapped.
fn map_again(host: &mut Host, id: u32, round: usize) -> u64 {
    assert!(host.wake(id));
    mapped_at(host, id, round)
}

/// Runs until cell `id` has printed exactly `out` and sleeps.
fn until_asleep_with(host: &mut Host, id: u32, out: &[u8]) {
    while host.output(id) != Some(out) || !host.asleep(id) {
        assert!(host.finished(id).is_none(), "{:?}", ended(host, id));
        host.step();
    }
}

/// Spec `cell-hierarchy`, "Same file in two domains": cells of two domains
/// map the same page of a private file both were given; each domain gets
/// its own frame, and the first domain's second map shares its first.
fn same_file_in_two_domains() {
    let mut files = rhizo_fs::Fs::new();
    files.put_file(b"/data", &[7u8; 4096]).expect("file");
    let mut host = Host::new();
    let (a, b) = (
        mapper(&mut host, &files, "/data", "1"),
        mapper(&mut host, &files, "/data", "1"),
    );
    let (da, db) = (host.domain(a).expect("a"), host.domain(b).expect("b"));
    assert_ne!(da, db, "two roots, two domains");
    let (at_a, at_b) = (mapped_at(&mut host, a, 1), mapped_at(&mut host, b, 1));
    let first = host.frame_of(a, at_a).expect("mapped");
    assert_ne!(host.frame_of(b, at_b), Some(first), "one frame per domain");
    assert_eq!((kept_pages_of(da), kept_pages_of(db)), (1, 1));
    let again = map_again(&mut host, a, 2);
    assert_eq!(
        host.frame_of(a, again),
        Some(first),
        "the domain's own frame"
    );
    assert_eq!(kept_pages_of(da), 1, "nothing filled again");
    assert!(host.kill(a) && host.kill(b));
}

/// Spec `cell-hierarchy`, "Library in two domains": the host's file sets
/// are published when decoded, their executables' pages loaded then;
/// cells of two domains map one of them on one frame, and a map after the
/// other cell ended is a hit too: nothing is ever filled for a domain. A
/// whole run of a dynamically linked program (its executable, `ld.so` and
/// the libraries `ld.so` maps) fills no page for its domain either, but
/// for its files that are not executables (`ld.so`'s cache). Only the sets
/// the host ships are published.
fn public_library_always_hits() {
    let files = crate::cell::dynamic_files().expect("xtask packs the dynamic hello");
    let &(path, ..) = expected::DYNAMIC.first().expect("a dynamic program");
    let public = kept_pages_of(PUBLIC);
    assert!(public > 0, "loaded when published");
    // A test fixture the host does not ship pins nothing: the distribution's
    // programs pinned 16 MiB, which made cells in parallel run out of memory
    // in a VM of 128 MiB.
    let _ = crate::cell::distro_files();
    assert_eq!(
        kept_pages_of(PUBLIC),
        public,
        "the fixture is not published"
    );
    let mut host = Host::new();
    let (a, b) = (
        mapper(&mut host, &files, path, "1"),
        mapper(&mut host, &files, path, "1"),
    );
    let (at_a, at_b) = (mapped_at(&mut host, a, 1), mapped_at(&mut host, b, 1));
    let frame = host.frame_of(a, at_a).expect("mapped");
    assert_eq!(host.frame_of(b, at_b), Some(frame), "one frame for all");
    assert!(host.kill(a));
    let again = map_again(&mut host, b, 2);
    assert_eq!(host.frame_of(b, again), Some(frame));
    let domains = [a, b].map(|c| host.domain(c).unwrap_or(c));
    assert_eq!(domains.map(kept_pages_of), [0, 0], "no page of a domain");
    assert_eq!(kept_pages_of(PUBLIC), public, "every map a hit");
    assert!(host.kill(b));
    let run = super::dynamic::spawn_dynamic(&mut host, &files, path, &["hello"]);
    host.run(|_| false);
    assert_eq!(ended(&host, run).0, Ending::Exit(0));
    let filled = kept_pages_of(run);
    assert!(
        filled <= 2,
        "{filled} pages filled for the program's domain"
    );
    assert_eq!(kept_pages_of(PUBLIC), public, "every map a hit");
}

/// Spec `isolation`, "Flushing another domain" (item 9 of design D12):
/// once a cell has filled its domain's part of the page cache, a cell of
/// another domain still gets its page kept, and its maps of it stay hits
/// on one frame while the first keeps mapping more than it may keep.
fn file_pages_kept_per_domain() {
    crate::cell::limit_kept_pages(Some(4));
    let mut files = rhizo_fs::Fs::new();
    files.put_file(b"/big", &[1u8; 16 * 4096]).expect("file");
    files.put_file(b"/lib", &[2u8; 4096]).expect("file");
    let mut host = Host::new();
    let flood = mapper(&mut host, &files, "/big", "16");
    mapped_at(&mut host, flood, 1);
    let full = host.domain(flood).expect("flood");
    assert_eq!(kept_pages_of(full), 4, "its domain's limit");
    let user = mapper(&mut host, &files, "/lib", "1");
    let at = mapped_at(&mut host, user, 1);
    let frame = host.frame_of(user, at).expect("mapped");
    let own = host.domain(user).expect("user");
    assert_eq!(kept_pages_of(own), 1, "kept, the other domain full");
    for round in 2..6 {
        map_again(&mut host, flood, round);
        let at = map_again(&mut host, user, round);
        assert_eq!(host.frame_of(user, at), Some(frame), "round {round}: a hit");
    }
    assert_eq!((kept_pages_of(full), kept_pages_of(own)), (4, 1));
    assert!(host.kill(flood) && host.kill(user));
    crate::cell::limit_kept_pages(None);
}

/// A `scoped write MIB` cell frozen into a template once it wrote its
/// pages; returns the template.
fn writer_template(host: &mut Host, mib: &[u8]) -> u32 {
    let id = spawn(host, "scoped", &[b"write", mib]);
    until_asleep_with(host, id, b"ready\n");
    assert!(host.freeze(id));
    id
}

/// Copy `id` of a template, not yet run, placed by the host as the root of
/// a new domain (protocol version 4 placement, design D13).
fn elsewhere(host: &mut Host, id: u32) -> u32 {
    host.place(id, HOST, Spec::default(), 0).expect("placed");
    id
}

/// Spec `control-plane`, "Publish and copy", spec `cell-hierarchy`,
/// "Unpublished template in another domain", and spec `fork`, "Fork of a
/// root": a copy of a template, and a fork of a root, stay in the source's
/// domain and share its frames; a copy of an unpublished template placed
/// in another domain shares none; copies of a published template placed
/// in other domains share its frames; after `withdraw`, a new copy placed
/// elsewhere shares none while the earlier ones run on.
fn publication_decides_what_copies_share() {
    let mut host = Host::new();
    let template = writer_template(&mut host, b"1");
    let frame = |host: &mut Host, id| host.frame_of(id, MMAP_BASE).expect("mapped");
    let copy = |host: &mut Host| {
        host.spawn_from_template(template, true, true)
            .expect("copy")
    };
    let in_domain = copy(&mut host);
    let fork = host.fork(in_domain, 1).expect("fork")[0];
    let private = copy(&mut host);
    let private = elsewhere(&mut host, private);
    assert_eq!(
        host.publish(template + 1000, false),
        Err(Refused::NoTemplate)
    );
    assert_eq!(host.publish(template, false), Ok(()));
    let shared = [0; 2].map(|_| {
        let id = copy(&mut host);
        elsewhere(&mut host, id)
    });
    assert!(host.withdraw(template) && !host.withdraw(template));
    let late = copy(&mut host);
    let late = elsewhere(&mut host, late);
    let domain = |host: &Host, id| host.domain(id).expect("in the tree");
    for id in [in_domain, fork] {
        assert_eq!(
            domain(&host, id),
            domain(&host, template),
            "{id}: same domain"
        );
    }
    let mut domains: Vec<u32> = [template, private, shared[0], shared[1], late]
        .iter()
        .map(|&id| domain(&host, id))
        .collect();
    domains.sort_unstable();
    domains.dedup();
    assert_eq!(domains.len(), 5, "each placed copy in a domain of its own");
    // Compared before the copies run: they would rewrite every page at once
    // (a template's deadlines expire in its copies).
    let of_template = frame(&mut host, in_domain);
    for id in [fork, shared[0], shared[1]] {
        assert_eq!(frame(&mut host, id), of_template, "{id}: shared");
    }
    for id in [private, late] {
        assert_ne!(frame(&mut host, id), of_template, "{id}: a full copy");
    }
    for id in [in_domain, fork, private, shared[0], shared[1], late] {
        until_asleep_with(&mut host, id, b"written\n");
        assert!(host.kill(id));
    }
    assert!(host.drop_template(template));
}

/// Item 12 of design D12: a published template dropped while copies in two
/// domains share it keeps its memory until its last copy ends, so a copy
/// never becomes the sole owner of a frame another domain shared: every
/// write of the last copy still copies. Then every frame comes back.
fn template_dropped_under_two_domains() {
    let before = memory_balance();
    let mut host = Host::new();
    let template = writer_template(&mut host, b"1");
    assert_eq!(host.publish(template, false), Ok(()));
    let [a, b] = [0; 2].map(|_| {
        let id = host
            .spawn_from_template(template, true, true)
            .expect("copy");
        elsewhere(&mut host, id)
    });
    assert_ne!(host.domain(a), host.domain(b));
    assert!(host.drop_template(template));
    assert!(host.kill(b), "b ends before a writes");
    let free = memory::free_bytes();
    until_asleep_with(&mut host, a, b"written\n");
    let copied = free.saturating_sub(memory::free_bytes());
    assert!(copied >= MIB, "every write copied: {copied} bytes");
    assert!(host.wake(a));
    host.run(|h| h.finished(a).is_some());
    assert_eq!(ended(&host, a), (Ending::Exit(0), "written\n".into()));
    drop(host);
    assert_eq!(memory_balance(), before, "gone with its last copy");
}

/// A root with 40 MiB to give children (a reserved root the smallest test
/// machine admits next to a promoted child: design D4 of
/// `add-cell-hierarchy`), and child `id` placed under it with 16 MiB.
fn parent_of(host: &mut Host, id: u32) -> u32 {
    let parent = spawn(host, "idle", &[]);
    let quota = Quota {
        memory: 40 * MIB,
        ..Quota::DEFAULT
    };
    assert!(host.set_limits(parent, quota, 8));
    let limits = Limits {
        memory: 16 * MIB,
        cpu_ns: u64::MAX,
        sockets: 16,
        cells: 0,
    };
    let spec = Spec {
        limits,
        ..Spec::default()
    };
    host.adopt(id, parent, spec, Actor::Host).expect("placed");
    parent
}

/// A `scoped write 2` cell placed under a parent, and its fork, a sibling
/// in the same domain sharing its memory copy-on-write: (parent, source,
/// sibling).
fn siblings(host: &mut Host) -> (u32, u32, u32) {
    let source = spawn(host, "scoped", &[b"write", b"2"]);
    let parent = parent_of(host, source);
    until_asleep_with(host, source, b"ready\n");
    let sibling = host.fork(source, 1).expect("fork")[0];
    assert_eq!(host.domain(sibling), host.domain(source), "in the domain");
    let frames = [source, sibling].map(|id| host.frame_of(id, MMAP_BASE));
    assert_eq!(frames[0], frames[1], "shared copy-on-write");
    (parent, source, sibling)
}

/// Wakes `scoped write` cell `id` and returns the bytes of memory taken by
/// its writes of every page, once it has printed `out`.
fn taken_by_writes(host: &mut Host, id: u32, out: &[u8]) -> u64 {
    let free = memory::free_bytes();
    assert!(host.wake(id));
    until_asleep_with(host, id, out);
    free.saturating_sub(memory::free_bytes())
}

/// Spec `cell-hierarchy`, "Promoted cell": a cell forked inside a domain
/// and promoted to root holds private copies of what it shared, so its
/// former sibling's writes and its own copy nothing.
fn promotion_unshares() {
    let before = memory_balance();
    let mut host = Host::new();
    let (parent, source, sibling) = siblings(&mut host);
    host.reparent(sibling, HOST, Actor::Host).expect("promoted");
    assert_ne!(host.domain(sibling), host.domain(source));
    let frames = [source, sibling].map(|id| host.frame_of(id, MMAP_BASE));
    assert_ne!(frames[0], frames[1], "private copies");
    let taken = taken_by_writes(&mut host, source, b"ready\nwritten\n");
    assert!(taken < NOISE, "the sibling's writes copied {taken} bytes");
    let taken = taken_by_writes(&mut host, sibling, b"written\n");
    assert!(
        taken < NOISE,
        "the promoted cell's writes copied {taken} bytes"
    );
    for id in [sibling, source, parent] {
        assert!(host.kill(id));
    }
    drop(host);
    assert_eq!(memory_balance(), before, "every frame back");
}

/// Spec `snapshots`, "Suspended cell moved by the host": a suspended fork
/// sharing pages with its source, promoted, and resumed reads its pages as
/// they were, and neither its writes nor the source's copy anything.
fn a_moved_snapshot_unshares() {
    let before = memory_balance();
    let mut host = Host::new();
    let (parent, source, sibling) = siblings(&mut host);
    let snapshot = host.suspend(sibling).expect("suspend");
    assert!(
        snapshot.shared_bytes() >= 2 * MIB,
        "it refers to its source's pages"
    );
    host.reparent(sibling, HOST, Actor::Host).expect("promoted");
    let resumed = host
        .resume(snapshot, Resume::Eager, true, true)
        .expect("resume");
    assert_ne!(host.domain(resumed), host.domain(source));
    let frames = [source, resumed].map(|id| host.frame_of(id, MMAP_BASE));
    assert_ne!(frames[0], frames[1], "private copies");
    let taken = taken_by_writes(&mut host, source, b"ready\nwritten\n");
    assert!(taken < NOISE, "the source's writes copied {taken} bytes");
    let taken = taken_by_writes(&mut host, resumed, b"written\n");
    assert!(
        taken < NOISE,
        "the resumed cell's writes copied {taken} bytes"
    );
    for id in [resumed, source, parent] {
        assert!(host.kill(id));
    }
    drop(host);
    assert_eq!(memory_balance(), before, "every frame back");
}

/// A snapshot of a template resumed is a copy of it, in its domain
/// (design D13): it shares the template's memory instead of copying it,
/// lazily or eagerly. A suspended fork moved into another domain and
/// resumed lazily first stops referring to its old domain's frames, then
/// pages in every page it needs (CPython's loaded copies faulted on one,
/// and copying a template's snapshot into a new domain each time ran a
/// 128 MiB VM out of memory).
fn a_template_snapshot_resumes_lazily_elsewhere() {
    let before = memory_balance();
    let mut host = Host::new();
    let template = writer_template(&mut host, b"2");
    let run_out = |host: &mut Host, copy: u32| {
        if host.asleep(copy) {
            assert!(host.wake(copy));
        }
        until_asleep_with(host, copy, b"written\n");
        assert!(host.wake(copy));
        host.run(|h| h.finished(copy).is_some());
        assert_eq!(ended(host, copy), (Ending::Exit(0), "written\n".into()));
    };
    for mode in [Resume::Lazy, Resume::Eager] {
        let snapshot = host.template_snapshot(template).expect("snapshot");
        assert!(snapshot.refers(), "it refers to the template's frames");
        let free = memory::free_bytes();
        let copy = host.resume(snapshot, mode, true, true).expect("resume");
        let taken = free.saturating_sub(memory::free_bytes());
        assert!(taken < NOISE, "{mode:?}: it shares, {taken} bytes taken");
        assert_eq!(host.domain(copy), host.domain(template), "{mode:?}");
        run_out(&mut host, copy);
    }
    assert!(host.drop_template(template));
    let (parent, source, sibling) = siblings(&mut host);
    let snapshot = host.suspend(sibling).expect("suspend");
    host.reparent(sibling, HOST, Actor::Host).expect("promoted");
    let moved = host
        .resume(snapshot, Resume::Lazy, true, true)
        .expect("resume");
    assert_ne!(host.domain(moved), host.domain(source));
    run_out(&mut host, moved);
    for id in [source, parent] {
        assert!(host.kill(id));
    }
    drop(host);
    assert_eq!(memory_balance(), before, "every frame back");
}
