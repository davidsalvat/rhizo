//! Executables kept loaded (change `add-image-cache`).

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(an_image_spawned_again_starts_fresh),
    ktest_case!(kept_images_give_way_to_cells),
    ktest_case!(a_cell_from_a_kept_image_suspends_resumes_and_forks),
];

/// An executable spawned again, from the space kept for it, starts with the
/// data in the executable, not what an earlier cell of it wrote; two cells
/// of it at once keep their writes to themselves.
fn an_image_spawned_again_starts_fresh() {
    let mut host = Host::new();
    let first = spawn(&mut host, "same_addr", &[b"first"]);
    host.run(|_| false);
    assert_eq!(
        ended(&host, first),
        (Ending::Exit(0), "own value ok\n".into())
    );
    let ids = [b"a", b"b"].map(|tag| spawn(&mut host, "same_addr", &[tag]));
    host.run(|_| false);
    for id in ids {
        assert_eq!(ended(&host, id), (Ending::Exit(0), "own value ok\n".into()));
    }
}

/// Short of frames, the executables kept loaded are dropped before a cell
/// is refused: with every frame cells may take taken, but for those kept,
/// a new cell still starts and runs; nothing leaks.
fn kept_images_give_way_to_cells() {
    let before = memory_balance();
    run_expected("libc_hello"); // kept loaded from now on
    run_expected("libc_float");
    // The frames taken are listed in themselves: the heap stays as it is.
    let mut taken = u64::MAX;
    while let Some(frame) = memory::alloc_for_cell(1) {
        DirectMap.write_u64(frame, taken);
        taken = frame;
    }
    let ran = crate::cell::run(crate::cell::test_image("hello"), &[b"hello"]);
    while taken != u64::MAX {
        let next = DirectMap.read_u64(taken);
        memory::with_frames(|f| f.free_contiguous(taken, 1));
        taken = next;
    }
    let (ending, output) = ran.expect("the cell starts");
    assert_eq!(ending, Ending::Exit(0));
    assert_eq!(output, b"hello from a cell\n");
    drop(output);
    assert_eq!(memory_balance(), before, "nothing leaked");
}

/// A cell started from a kept executable is a cell like any other: it is
/// suspended, resumed and forked, and every copy runs to its end.
fn a_cell_from_a_kept_image_suspends_resumes_and_forks() {
    let mut host = Host::new();
    let first = forkme_ready(&mut host);
    assert!(host.kill(first)); // `forkme` is kept loaded from now on
    let parent = forkme_ready(&mut host);
    let snapshot = host.suspend(parent).expect("live");
    let resumed = host
        .resume(snapshot, Resume::Lazy, true, true)
        .expect("resume");
    let children = host.fork(resumed, 2).expect("fork");
    host.run(|_| false);
    for id in [resumed].into_iter().chain(children) {
        assert_eq!(ended(&host, id), (Ending::Exit(0), "own ok\n".into()));
    }
}
