//! The cell hierarchy (spec `cell-hierarchy`, change `add-cell-hierarchy`).

use super::*;
use rhizo_cap::{Actor, Egress, Error as TreeError, HOST, Limits, OnParentEnd, Spec};
use rhizo_cell::quota::Quota;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(identifiers_are_per_domain),
    ktest_case!(tree_operations),
    ktest_case!(cascade_and_promote),
];

const MIB: u64 = 1 << 20;

/// A root's quota of `mib` MiB.
fn quota(mib: u64) -> Quota {
    Quota {
        memory: mib * MIB,
        ..Quota::DEFAULT
    }
}

/// A child's limits: `mib` MiB, 16 sockets, `cells` descendants.
fn child(mib: u64, cells: u32) -> Spec {
    Spec {
        limits: Limits {
            memory: mib * MIB,
            cpu_ns: u64::MAX,
            sockets: 16,
            cells,
        },
        ..Spec::default()
    }
}

/// A cell that sleeps until it is ended.
fn idle(host: &mut Host) -> u32 {
    spawn(host, "idle", &[])
}

/// Spec `linux-abi`, "Identifiers per domain": `getpid` and the first
/// thread's `gettid` count from 1 in each domain, whatever other domains
/// made and ended meanwhile (they counted every cell ever made, finding O6).
/// A copy takes another number than its source, and so does a resumed
/// cell (a resume is a copy, design D22), in its own domain.
fn identifiers_are_per_domain() {
    let mut host = Host::new();
    let first = spawn(&mut host, "observer", &[b"ids"]);
    host.run(|_| false);
    for _ in 0..500 {
        let busy = spawn(&mut host, "observer", &[b"ids"]);
        assert!(host.kill(busy));
    }
    let second = spawn(&mut host, "observer", &[b"ids"]);
    host.run(|_| false);
    for id in [first, second] {
        let out = "pid 1 tid 1\npid 1 tid 1\n";
        assert_eq!(ended(&host, id), (Ending::Exit(0), out.into()));
    }
    let source = spawn(&mut host, "observer", &[b"ids"]);
    while host.output(source) != Some(b"pid 1 tid 1\n") {
        host.step();
    }
    let copy = host.fork(source, 1).expect("fork")[0];
    let domains = [host.domain(source), host.domain(copy)];
    assert!(domains[0].is_some_and(|d| d != 0), "{domains:?}");
    assert_ne!(domains[0], domains[1], "a copy of a root is a root");
    let snapshot = host.suspend(source).expect("suspend");
    let resumed = host
        .resume(snapshot, Resume::Eager, true, false)
        .expect("resume");
    assert_eq!(host.domain(resumed), domains[0], "the domain stays");
    host.run(|_| false);
    // The copy keeps its threads' identifiers: its program may hold them.
    assert_eq!(
        ended(&host, copy),
        (Ending::Exit(0), "pid 2 tid 1\n".into())
    );
    let out = "pid 2 tid 1\n";
    assert_eq!(ended(&host, resumed), (Ending::Exit(0), out.into()));
    assert!(host.tree().is_empty(), "every node went with its cell");
}

/// Spec `cell-hierarchy`: the host places cells under others ("Children
/// of a cell"), suspending a parent leaves its children running, a cell
/// never acts on itself, its parent, a sibling or another domain ("A child
/// turns on its parent", "Moves that are refused"), refused operations
/// change nothing, and a parent promoting its child gives up the child's
/// reservation ("Parent promotes a child").
fn tree_operations() {
    let mut host = Host::new();
    let parent = idle(&mut host);
    assert!(host.set_limits(parent, quota(64), 8));
    let first = spawn(&mut host, "observer", &[b"ids"]);
    host.adopt(first, parent, child(16, 2), Actor::Host)
        .expect("placed");
    let second = idle(&mut host);
    host.adopt(second, parent, child(8, 0), Actor::Host)
        .expect("placed");
    let other = idle(&mut host);
    let spare = idle(&mut host);
    assert_eq!(host.tree().get(first).map(|n| n.parent()), Some(parent));
    assert_eq!(host.domain(first), host.domain(parent));
    assert_ne!(host.domain(other), host.domain(parent));

    // Ownership is not scheduling: the child runs on while its parent is
    // suspended, numbered in its parent's domain.
    let snapshot = host.suspend(parent).expect("suspend");
    host.run(|h| h.finished(first).is_some());
    let out = "pid 2 tid 2\npid 2 tid 2\n";
    assert_eq!(ended(&host, first), (Ending::Exit(0), out.into()));
    let parent = host
        .resume(snapshot, Resume::Eager, true, false)
        .expect("resume");
    assert_eq!(host.tree().get(second).map(|n| n.parent()), Some(parent));

    let before = host.tree().clone();
    let refused = [
        host.reparent(parent, second, Actor::Host),
        host.reparent(second, HOST, Actor::Cell(second)),
        host.kill_tree(parent, Actor::Cell(second)),
        host.suspend_tree(parent, Actor::Cell(second)).map(|_| ()),
        host.reparent(second, other, Actor::Cell(parent)),
        host.adopt(spare, parent, child(1, 0), Actor::Cell(parent)),
        host.adopt(spare, parent, child(64, 0), Actor::Host),
        host.adopt(
            spare,
            parent,
            Spec {
                egress: Egress::Full,
                ..child(1, 0)
            },
            Actor::Host,
        ),
    ];
    use TreeError::*;
    let reasons = [
        Cycle, Denied, Denied, Denied, Denied, Denied, Budget, Denied,
    ];
    assert_eq!(refused, reasons.map(Err));
    assert_eq!(*host.tree(), before, "a refused operation changes nothing");
    assert_eq!(host.live(), 4, "and every cell runs on");

    // The 16 MiB child ended; promoting the 8 MiB one leaves 56 MiB.
    host.reparent(second, HOST, Actor::Cell(parent))
        .expect("promoted");
    let (p, c) = (host.tree().get(parent), host.tree().get(second));
    assert_eq!(p.map(|n| n.limits().memory), Some(56 * MIB));
    assert_eq!(
        c.map(|n| (n.is_root(), n.limits().memory, n.pid())),
        Some((true, 8 * MIB, 1))
    );
    assert_ne!(host.domain(second), host.domain(parent));

    for cell in [parent, second, other, spare] {
        host.kill_tree(cell, Actor::Host).expect("ended");
    }
    host.run(|_| false);
    assert!(host.tree().is_empty());
}

/// Spec `cell-hierarchy`, "Cascade" and "Promote": a parent with a running
/// child (with a child of its own), a suspended one and a frozen one ends,
/// and so do they, their template gone; its child with the policy
/// `promote` runs on as a root of its own domain. Every frame comes back.
fn cascade_and_promote() {
    fn round() {
        let mut host = Host::new();
        let parent = idle(&mut host);
        assert!(host.set_limits(parent, quota(64), 8));
        let place = |host: &mut Host, under, spec| {
            let cell = idle(host);
            host.adopt(cell, under, spec, Actor::Host).expect("placed");
            cell
        };
        let running = place(&mut host, parent, child(8, 1));
        let grandchild = place(&mut host, running, child(2, 0));
        let suspended = place(&mut host, parent, child(8, 0));
        let frozen = place(&mut host, parent, child(8, 0));
        let keep = Spec {
            on_parent_end: OnParentEnd::Promote,
            ..child(8, 0)
        };
        let kept = place(&mut host, parent, keep);
        host.run_until(crate::cell::now_ns() + 50_000_000, |_| false);
        let snapshot = host.suspend(suspended).expect("suspend");
        assert!(host.freeze(frozen));
        assert!(host.kill(parent));
        let ends = host.take_finished();
        for cell in [running, grandchild, suspended] {
            let killed = ends
                .iter()
                .any(|f| f.id == cell && f.ending == Ending::Killed);
            assert!(killed, "cell {cell} ended with its parent");
        }
        assert_eq!(host.spawn_from_template(frozen, false, false), None);
        let k = host.tree().get(kept).expect("promoted");
        assert!(
            k.is_root() && k.pid() == 1,
            "a root numbered 1 in its domain"
        );
        assert_eq!(host.live(), 1, "the promoted cell runs on");
        drop(snapshot);
        assert!(host.kill(kept));
        assert!(host.tree().is_empty());
    }
    round();
    let before = free_or_in_heap();
    round();
    assert_eq!(free_or_in_heap(), before, "every frame came back");
}
