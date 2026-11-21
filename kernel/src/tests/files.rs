//! Files (H8).

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(files_behave_as_on_linux_and_return_memory),
    ktest_case!(files_are_private_to_each_cell),
    ktest_case!(initial_files_are_given_at_spawn),
    ktest_case!(open_file_survives_suspend_and_resume),
    ktest_case!(forked_cells_have_their_own_files),
    ktest_case!(template_files_are_inherited),
    ktest_case!(file_data_takes_its_own_size),
];

/// A real program using files through glibc prints what it prints on Linux,
/// and an ended cell's files and descriptors return all their memory.
fn files_behave_as_on_linux_and_return_memory() {
    assert_no_leak("libc_files");
    assert_no_leak("diary");
}

fn files_are_private_to_each_cell() {
    let mut host = Host::new();
    let a = spawn(&mut host, "diary", &[b"first"]);
    let b = spawn(&mut host, "diary", &[b"second"]);
    host.run(|_| false);
    for id in [a, b] {
        // Both write `diary.txt`; each checks the file holds only its writes.
        assert_eq!(
            ended(&host, id),
            (Ending::Exit(0), "ready\nown ok\n".into())
        );
    }
}

fn initial_files_are_given_at_spawn() {
    let mut files = rhizo_fs::Fs::new();
    files
        .put_file(b"/input.txt", b"from the host")
        .expect("put");
    let mut host = Host::new();
    let id = host
        .spawn_with_files(
            crate::cell::test_image("diary"),
            &[b"diary"],
            files,
            true,
            true,
        )
        .expect("spawn");
    host.run(|_| false);
    assert_eq!(
        ended(&host, id),
        (
            Ending::Exit(0),
            "input from the host\nready\nown ok\n".into()
        )
    );
}

/// Suspended after every step with `diary.txt` open, and resumed from the
/// snapshot's bytes: the descriptor, its offset and the data all come back.
fn open_file_survives_suspend_and_resume() {
    for mode in [Resume::Eager, Resume::Lazy] {
        let mut host = Host::new();
        let mut id = spawn(&mut host, "diary", &[]);
        let mut output = Vec::new();
        let mut resumes = 0;
        loop {
            host.step();
            if let Some(done) = host.finished(id) {
                assert_eq!(done.ending, Ending::Exit(0));
                output.extend_from_slice(&done.output);
                break;
            }
            let snapshot = host.suspend(id).expect("live");
            output.extend_from_slice(&host.finished(id).expect("suspended").output);
            let snapshot = Snapshot::from_bytes(
                snapshot.export(&DirectMap, crate::arch::mem::crc32c),
                crate::arch::user::MACHINE,
                crate::arch::mem::crc32c,
            )
            .expect("valid");
            id = host.resume(snapshot, mode, true, false).expect("resume");
            resumes += 1;
        }
        assert!(resumes >= 4, "suspended {resumes} times");
        assert_eq!(output, b"ready\nown ok\n");
    }
}

/// Spawns `diary` and runs it until its file is written and it yields.
fn diary_ready(host: &mut Host, tag: &[u8]) -> u32 {
    let id = spawn(host, "diary", &[tag]);
    while host.output(id) != Some(b"ready\n") {
        host.step();
    }
    id
}

fn forked_cells_have_their_own_files() {
    let mut host = Host::new();
    let parent = diary_ready(&mut host, b"shared");
    let children = host.fork(parent, 3).expect("fork");
    host.run(|_| false);
    assert_eq!(
        ended(&host, parent),
        (Ending::Exit(0), "ready\nown ok\n".into())
    );
    for child in children {
        // Each found the parent's `shared:` and only its own identifier.
        assert_eq!(ended(&host, child), (Ending::Exit(0), "own ok\n".into()));
    }
}

fn template_files_are_inherited() {
    let mut host = Host::new();
    let template = diary_ready(&mut host, b"template");
    assert!(host.freeze(template));
    let ids: Vec<u32> = (0..3)
        .map(|_| {
            host.spawn_from_template(template, true, true)
                .expect("spawn")
        })
        .collect();
    host.run(|_| false);
    for id in ids {
        assert_eq!(ended(&host, id), (Ending::Exit(0), "own ok\n".into()));
    }
}

/// A cell holding a 16 MiB file uses less than 17 MiB, its own memory
/// included: a 4 KiB block takes one page, not two.
fn file_data_takes_its_own_size() {
    let mut host = Host::new();
    let warm = spawn(&mut host, "filer", &[]);
    host.run(|h| h.finished(warm).is_some()); // the heap grows its classes once
    let before = memory::free_bytes();
    let id = spawn(&mut host, "filer", &[b"16"]);
    while host.output(id) != Some(b"files ok\n") {
        host.step();
    }
    let used = before - memory::free_bytes();
    assert!(used >= 16 << 20, "the file is held: {used} bytes");
    assert!(used < 17 << 20, "{used} bytes for 16 MiB of file data");
    host.run(|_| false);
}
