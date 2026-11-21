//! The cell hierarchy (spec `cell-hierarchy`, change `add-cell-hierarchy`).

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[ktest_case!(identifiers_are_per_domain)];

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
