//! Packages from PyPI (C.2), alone in the kernel built with the packages
//! embedded (change `add-pip-packages`), whose VM has the memory they need.
//! requests first: the network card gets its address there, which takes
//! memory for good, before numpy's test checks that none is left behind.

use super::dynamic::spawn_dynamic;
use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(console_output_works),
    ktest_case!(requests_gets_from_the_host_as_on_linux),
    ktest_case!(numpy_from_a_wheel_computes_as_on_linux),
    ktest_case!(a_cell_that_imported_numpy_suspends_resumes_and_forks),
];

/// The files of the packages' CPython, or `None` (said on the console)
/// when they were not packed.
fn packages() -> Option<rhizo_fs::Fs> {
    let files = crate::cell::packages_files();
    if files.is_none() {
        arch::console::BootConsole.write_bytes(b"(packages not packed: not run) ");
    }
    files
}

/// Spawns the packages' CPython with `argv` (spec `workloads`, packages
/// from PyPI).
fn spawn_python(host: &mut Host, files: &rhizo_fs::Fs, argv: &[&str]) -> u32 {
    spawn_dynamic(host, files, expected::NUMPY_ARGV[0], argv)
}

/// numpy from its manylinux wheel: CPython opens its extension modules and
/// the OpenBLAS they need with `dlopen`, and the computation prints what it
/// prints on Linux (`cells/expected.rs`, by the differential check). Once
/// the kept file pages are dropped, every frame and heap allocation is
/// back.
fn numpy_from_a_wheel_computes_as_on_linux() {
    let Some(files) = packages() else { return };
    let before = memory_balance();
    let mut host = Host::new();
    let id = spawn_python(&mut host, &files, &expected::NUMPY_ARGV);
    host.run(|h| h.finished(id).is_some());
    assert_eq!(
        ended(&host, id),
        (Ending::Exit(0), expected::NUMPY_STDOUT.into())
    );
    drop(host);
    assert!(crate::cell::kept_pages() > 0, "file pages were kept");
    assert_eq!(memory_balance(), before, "nothing leaked");
}

/// requests (with urllib3, idna, charset_normalizer and certifi) makes an
/// HTTP GET to the host's fake model through the network card and prints
/// what a Linux process prints of the answer.
fn requests_gets_from_the_host_as_on_linux() {
    let Some(files) = packages() else { return };
    let Some((ip, port)) = crate::wire::options().llm else {
        arch::console::BootConsole.write_bytes(b"(no network card and model server: not run) ");
        return;
    };
    let mut host = Host::new();
    assert!(
        host.wire_address(10_000_000_000).is_some(),
        "the card got an address"
    );
    let url = alloc::format!("http://{}.{}.{}.{}:{port}", ip[0], ip[1], ip[2], ip[3]);
    let [python, flag, script] = expected::REQUESTS_ARGV;
    let id = spawn_python(&mut host, &files, &[python, flag, script, &url]);
    let end = crate::cell::now_ns() + 600_000_000_000;
    while host.finished(id).is_none() && crate::cell::now_ns() < end {
        host.step();
    }
    assert_eq!(
        ended(&host, id),
        (Ending::Exit(0), expected::REQUESTS_STDOUT.into())
    );
    drop(host);
    assert_eq!(crate::cell::open_sockets(), 0);
    // Its connection finishes closing with the host: the kernel goes back
    // to holding nothing for it.
    settle_wire();
}

/// The template case: a cell that imported numpy, suspended and resumed
/// from the snapshot's bytes, forked, and frozen as a template, every copy
/// finishes the computation without importing numpy again.
fn a_cell_that_imported_numpy_suspends_resumes_and_forks() {
    let Some(files) = packages() else { return };
    let [python, flag, script] = expected::NUMPY_ARGV;
    let mut host = Host::new();
    let ready = |host: &mut Host| {
        let id = spawn_python(host, &files, &[python, flag, script, "wait"]);
        host.run_to_pause(id, b"ready", Pause::Slice)
            .unwrap_or_else(|why| panic!("{why}"));
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
    let all: Vec<u32> = [resumed]
        .into_iter()
        .chain(children)
        .chain(from_template)
        .collect();
    host.run(|h| all.iter().all(|&id| h.finished(id).is_some()));
    for id in all {
        assert_eq!(
            ended(&host, id),
            (Ending::Exit(0), expected::NUMPY_STDOUT.into())
        );
    }
}
