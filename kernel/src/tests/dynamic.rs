//! Dynamic linking (C.1).

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(dynamically_linked_programs_behave_as_on_linux),
    ktest_case!(a_dynamically_linked_cell_suspends_resumes_and_forks),
];

/// Spawns the program at `path` in `files` with `argv` (a
/// [`expected::DYNAMIC`] entry).
pub(super) fn spawn_dynamic(
    host: &mut Host,
    files: &rhizo_fs::Fs,
    path: &str,
    argv: &[&str],
) -> u32 {
    let argv: Vec<&[u8]> = argv.iter().map(|a| a.as_bytes()).collect();
    host.spawn_file(files.clone(), path.as_bytes(), &argv, &[], true, true)
        .unwrap_or_else(|e| panic!("cannot spawn {path}: {e:?}"))
}

/// The dynamically linked programs `xtask` packs (spec `cells`,
/// dynamically linked executables): each, started from the cell's files by
/// its interpreter, prints what it prints on Linux (`cells/expected.rs`, by
/// the differential check) and exits with 0. Their file pages are kept for
/// later cells, and once dropped every frame and heap allocation is back.
fn dynamically_linked_programs_behave_as_on_linux() {
    let packed = [crate::cell::dynamic_files(), crate::cell::distro_files()];
    assert!(packed[0].is_some(), "xtask packs the dynamic hello");
    let before = memory_balance();
    for &(path, argv, stdout) in expected::DYNAMIC {
        let held = |f: &&rhizo_fs::Fs| f.file_at(path.as_bytes()).is_ok();
        let Some(files) = packed.iter().flatten().find(held) else {
            let note = alloc::format!("({path} not packed: not run) ");
            arch::console::BootConsole.write_bytes(note.as_bytes());
            continue;
        };
        let mut host = Host::new();
        let id = spawn_dynamic(&mut host, files, path, argv);
        host.run(|_| false);
        assert_eq!(ended(&host, id), (Ending::Exit(0), stdout.into()), "{path}");
    }
    assert!(crate::cell::drop_images(), "file pages were kept");
    assert_eq!(memory_balance(), before, "nothing leaked");
}

/// A dynamically linked cell is plain data like any other (spec `cells`):
/// suspended once its interpreter has started it, resumed from the
/// snapshot's bytes, forked, and frozen as a template, every copy finishes
/// the program.
fn a_dynamically_linked_cell_suspends_resumes_and_forks() {
    let files = crate::cell::dynamic_files().expect("xtask packs the dynamic hello");
    let &(path, argv, stdout) = expected::DYNAMIC
        .iter()
        .find(|p| p.1 == ["hello", "wait"])
        .expect("hello wait");
    let rest = stdout.strip_prefix("ready\n").expect("ready first");
    let mut host = Host::new();
    let ready = |host: &mut Host| {
        let id = spawn_dynamic(host, &files, path, argv);
        while host.output(id) != Some(b"ready\n") {
            host.step();
        }
        id
    };
    let id = ready(&mut host);
    let bytes = host
        .suspend(id)
        .expect("live")
        .export(&DirectMap, crate::arch::mem::crc32c);
    let snapshot =
        Snapshot::from_bytes(bytes, crate::arch::user::MACHINE, crate::arch::mem::crc32c)
            .expect("valid");
    let resumed = host
        .resume(snapshot, Resume::Lazy, true, true)
        .expect("resume");
    let children = host.fork(resumed, 2).expect("fork");
    let template = ready(&mut host);
    assert!(host.freeze(template));
    let from_template: Vec<u32> = (0..2)
        .map(|_| {
            host.spawn_from_template(template, true, true)
                .expect("fork")
        })
        .collect();
    host.run(|_| false);
    for id in [resumed].into_iter().chain(children).chain(from_template) {
        assert_eq!(ended(&host, id), (Ending::Exit(0), rest.into()));
    }
}
