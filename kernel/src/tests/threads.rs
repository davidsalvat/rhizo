//! Threads (H14).

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(threads_behave_as_on_linux_and_return_memory),
    ktest_case!(spinning_thread_does_not_starve_its_siblings),
    ktest_case!(thread_count_does_not_buy_cpu_time),
    ktest_case!(cells_whose_threads_all_wait_use_no_cpu, 293),
    ktest_case!(threaded_cell_survives_suspend_and_resume, 95),
    ktest_case!(forks_of_a_threaded_cell_run_all_threads),
    ktest_case!(template_of_a_threaded_program),
];

/// Raw `clone` and futexes (`threads`) and glibc's pthreads, mutexes and
/// condition variables (`libc_threads`) behave as on Linux, and a threaded
/// cell returns every frame: thread stacks, arenas, all of it.
fn threads_behave_as_on_linux_and_return_memory() {
    assert_no_leak("threads");
    assert_no_leak("libc_threads");
}

/// A thread prints and calls `exit_group` while the first spins: the cell
/// ends with every thread, and all its memory comes back.
fn spinning_thread_does_not_starve_its_siblings() {
    fn round() {
        let mut host = Host::new();
        let id = spawn(&mut host, "threads", &[b"starve"]);
        host.run(|h| h.finished(id).is_some());
        assert_eq!(ended(&host, id), (Ending::Exit(0), "not starved\n".into()));
        assert_eq!(host.live(), 0);
    }
    round();
    let before = free_or_in_heap();
    round();
    assert_eq!(free_or_in_heap(), before, "every frame returned");
}

/// Scheduling steps until a `chatter` cell ends next to a cell of `n`
/// spinning threads.
fn steps_next_to_spinners(n: &[u8]) -> usize {
    let mut host = Host::new();
    let spinner = spawn(&mut host, "threads", &[b"spin", n]);
    let chatter = spawn(&mut host, "chatter", &[]);
    let mut steps = 0;
    while host.finished(chatter).is_none() {
        host.step();
        steps += 1;
    }
    assert!(host.kill(spinner));
    steps
}

fn thread_count_does_not_buy_cpu_time() {
    let one = steps_next_to_spinners(b"1");
    let four = steps_next_to_spinners(b"4");
    assert_eq!(four, one, "the cell, not the thread, gets the time");
}

fn cells_whose_threads_all_wait_use_no_cpu() {
    let mut host = Host::new();
    let parked = spawn(&mut host, "threads", &[b"park"]);
    let hello = spawn(&mut host, "hello", &[]);
    host.run(|_| false); // returns: only a cell that can never run is left
    assert_eq!(
        ended(&host, hello),
        (Ending::Exit(0), "hello from a cell\n".into())
    );
    assert_eq!(host.live(), 1, "the waiting cell is still live");
    assert!(host.finished(parked).is_none());
    // Its threads still wait after a suspend and resume, and it can be killed.
    let snapshot = host.suspend(parked).expect("live");
    let parked = host
        .resume(snapshot, Resume::Lazy, true, false)
        .expect("resume");
    host.run(|_| false);
    assert!(host.kill(parked));
    assert_eq!(ended(&host, parked).0, Ending::Killed);
}

pub(super) fn expected_output(name: &str) -> &'static str {
    expected::PROGRAMS
        .iter()
        .find(|p| p.0 == name)
        .unwrap_or_else(|| panic!("{name} is not in cells/expected.rs"))
        .1
}

/// `libc_threads` suspended after every scheduling step and resumed from
/// the snapshot's bytes (threads waiting on condition variables included),
/// eagerly and lazily: its output is the uninterrupted one.
fn threaded_cell_survives_suspend_and_resume() {
    for mode in [Resume::Eager, Resume::Lazy] {
        // Progress for the harness: the run prints nothing else meanwhile.
        arch::console::BootConsole.write_bytes(if mode == Resume::Eager {
            b"(eager) "
        } else {
            b"(lazy) "
        });
        let mut host = Host::new();
        let mut id = spawn(&mut host, "libc_threads", &[]);
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
        assert!(resumes >= 20, "suspended {resumes} times");
        assert_eq!(
            alloc::string::String::from_utf8_lossy(&output),
            expected_output("libc_threads")
        );
    }
}

/// Spawns `libc_threads` and runs it until its workers wait for the go.
fn threaded_ready(host: &mut Host) -> u32 {
    let id = spawn(host, "libc_threads", &[]);
    while host.output(id) != Some(b"ready\n") {
        host.step();
    }
    id
}

fn forks_of_a_threaded_cell_run_all_threads() {
    let after_ready = expected_output("libc_threads")
        .strip_prefix("ready\n")
        .expect("ready first");
    let mut host = Host::new();
    let parent = threaded_ready(&mut host);
    let children = host.fork(parent, 2).expect("fork");
    host.run(|_| false);
    assert_eq!(
        ended(&host, parent),
        (Ending::Exit(0), expected_output("libc_threads").into())
    );
    for child in children {
        assert_eq!(ended(&host, child), (Ending::Exit(0), after_ready.into()));
    }
}

fn template_of_a_threaded_program() {
    let after_ready = expected_output("libc_threads")
        .strip_prefix("ready\n")
        .expect("ready first");
    let mut host = Host::new();
    let template = threaded_ready(&mut host);
    assert!(host.freeze(template));
    let ids: Vec<u32> = (0..3)
        .map(|_| {
            host.spawn_from_template(template, true, true)
                .expect("spawn")
        })
        .collect();
    host.run(|_| false);
    for id in ids {
        assert_eq!(ended(&host, id), (Ending::Exit(0), after_ready.into()));
    }
}
