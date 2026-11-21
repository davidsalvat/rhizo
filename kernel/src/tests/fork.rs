//! Fork and templates (spec `fork`).

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(forked_cells_see_only_their_own_writes),
    ktest_case!(children_outlive_their_parent_and_frames_return),
    ktest_case!(forks_of_forks),
    ktest_case!(fork_allocates_only_a_root),
    ktest_case!(spawn_from_template_runs_like_the_program),
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
