//! Code written at run time (change `node-jit`).

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(code_written_at_run_time_runs_as_on_linux),
    ktest_case!(
        code_written_at_run_time_survives_fork_suspend_and_templates,
        41
    ),
    ktest_case!(code_written_at_run_time_survives_repeated_suspends, 71),
    ktest_case!(code_memory_misused_is_a_fault),
];

/// A C program that writes code into writable and executable memory, runs
/// it, rewrites it, reads and has a system call fill code memory it never
/// touched, and shares a code page between two threads, in turns and at
/// once, prints what it prints on Linux (spec `workloads`, design D9 of
/// `node-jit`) and returns every frame. The changes between writing and
/// running are counted and printed (design D4).
fn code_written_at_run_time_runs_as_on_linux() {
    crate::prof::start();
    assert_no_leak("libc_jit");
    let flips = crate::prof::stop().count(crate::prof::Count::WxFlips);
    // Two runs, each making the page running once per turn of its threads.
    assert!(flips >= 2 * 200, "{flips} changes");
    let said = alloc::format!("({flips} flips) ");
    arch::console::BootConsole.write_bytes(said.as_bytes());
}

/// The output of `libc_jit` after `ready`.
fn jit_after_ready() -> &'static str {
    let all = expected_output("libc_jit");
    &all[all.find("ready\n").expect("ready") + "ready\n".len()..]
}

/// Spawns `libc_jit` and runs it until it prints `ready`.
fn jit_ready(host: &mut Host) -> u32 {
    let id = spawn(host, "libc_jit", &[]);
    while !host.output(id).is_some_and(|o| o.ends_with(b"ready\n")) {
        assert!(host.finished(id).is_none(), "{:?}", ended(host, id));
        host.step();
    }
    id
}

/// Forked, suspended and resumed (eagerly and lazily), or frozen as a
/// template once it ran code it wrote, every copy of `libc_jit` writes and
/// runs its own code, finds the rest of its ranges zeroed on first touch,
/// and prints the rest of its output (specs `fork`, `snapshots`,
/// `isolation`; design D6 and D7 of `node-jit`).
fn code_written_at_run_time_survives_fork_suspend_and_templates() {
    let mut host = Host::new();
    let parent = jit_ready(&mut host);
    let children = host.fork(parent, 2).expect("fork");
    let mut resumed = Vec::new();
    for mode in [Resume::Eager, Resume::Lazy] {
        let id = jit_ready(&mut host);
        let bytes = host
            .suspend(id)
            .expect("live")
            .export(&DirectMap, crate::arch::mem::crc32c);
        let snapshot =
            Snapshot::from_bytes(bytes, crate::arch::user::MACHINE, crate::arch::mem::crc32c)
                .expect("valid");
        resumed.push(host.resume(snapshot, mode, true, true).expect("resume"));
    }
    let template = jit_ready(&mut host);
    assert!(host.freeze(template));
    let from_template: Vec<u32> = (0..2)
        .map(|_| {
            host.spawn_from_template(template, true, true)
                .expect("spawn")
        })
        .collect();
    host.run(|_| false);
    assert_eq!(
        ended(&host, parent),
        (Ending::Exit(0), expected_output("libc_jit").into())
    );
    for id in children.into_iter().chain(resumed).chain(from_template) {
        assert_eq!(
            ended(&host, id),
            (Ending::Exit(0), jit_after_ready().into()),
            "cell {id}"
        );
    }
}

/// `libc_jit` suspended after every few scheduling steps (every one would
/// take minutes: its threads take hundreds of turns) and resumed lazily
/// from the snapshot's bytes, its code pages writing or running at each
/// stop: its output is the uninterrupted one (spec `snapshots`).
fn code_written_at_run_time_survives_repeated_suspends() {
    const EVERY: u32 = 8;
    let mut host = Host::new();
    let mut id = spawn(&mut host, "libc_jit", &[]);
    let mut output = Vec::new();
    let (mut resumes, mut steps) = (0, 0u32);
    loop {
        host.step();
        if let Some(done) = host.finished(id) {
            assert_eq!(done.ending, Ending::Exit(0));
            output.extend_from_slice(&done.output);
            break;
        }
        steps += 1;
        if !steps.is_multiple_of(EVERY) {
            continue;
        }
        let snapshot = host.suspend(id).expect("live");
        output.extend_from_slice(&host.finished(id).expect("suspended").output);
        let snapshot = Snapshot::from_bytes(
            snapshot.export(&DirectMap, crate::arch::mem::crc32c),
            crate::arch::user::MACHINE,
            crate::arch::mem::crc32c,
        )
        .expect("valid");
        id = host
            .resume(snapshot, Resume::Lazy, true, false)
            .expect("resume");
        resumes += 1;
    }
    assert!(resumes >= 5, "suspended {resumes} times");
    assert_eq!(
        alloc::string::String::from_utf8_lossy(&output),
        expected_output("libc_jit")
    );
}

/// Code that stores into the write-or-execute page it runs from can never
/// complete without the page writable and executable at once: the cell is
/// killed with a fault report, and another cell runs on; so is a cell that
/// touches code memory it unmapped (spec `linux-abi`).
fn code_memory_misused_is_a_fault() {
    let mut host = Host::new();
    let id = spawn(&mut host, "libc_jit", &[b"self"]);
    let other = spawn(&mut host, "hello", &[]);
    host.run(|_| false);
    let (ending, said) = ended(&host, id);
    assert_eq!(ending, Ending::Fault, "{said}");
    assert!(
        said.starts_with("self-modifying\n") && !said.contains("survived"),
        "{said}"
    );
    assert_eq!(
        ended(&host, other),
        (Ending::Exit(0), "hello from a cell\n".into())
    );
    // Part of untouched code memory unmapped: the rest still comes zeroed,
    // the part is gone (spec `linux-abi`, write-or-execute ranges).
    let id = spawn(&mut host, "libc_jit", &[b"unmapped"]);
    host.run(|_| false);
    let (ending, said) = ended(&host, id);
    assert_eq!(ending, Ending::Fault, "{said}");
    assert!(
        said.starts_with("rest zero\n") && !said.contains("survived"),
        "{said}"
    );
}
