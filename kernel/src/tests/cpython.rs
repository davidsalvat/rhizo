//! CPython.

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(stack_grows_on_touch_and_returns_its_frames),
    ktest_case!(cpython_in_a_cell),
    ktest_case!(a_cpython_agent_talks_forks_and_survives_suspend),
];

/// A program using 4 MiB of stack, and a system call writing into stack it
/// never touched, run as on Linux: the stack grows a page at a time up to
/// its 8 MiB limit, and every page comes back when the cell ends (design D5
/// of `add-cpython`).
fn stack_grows_on_touch_and_returns_its_frames() {
    assert_no_leak("libc_deep_stack");
}

/// CPython running `cells/python/agent_demo.py` with its standard library
/// in the cell's files prints what it prints on Linux; frozen after its
/// warm-up as a template, every cell forked from it answers without warming
/// up again. Built only when its source is given (`RHIZO_CPYTHON_SRC`, set
/// in CI); otherwise the test says it did not run.
fn cpython_in_a_cell() {
    let image = crate::cell::test_image("cpython");
    let Some(files) = crate::cell::cpython_files() else {
        arch::console::BootConsole.write_bytes(b"(CPython not built: not run) ");
        return;
    };
    let argv = expected::CPYTHON_ARGV.map(str::as_bytes);
    let mut host = Host::new();
    let id = host
        .spawn_with_files(image, &argv, files.clone(), true, true)
        .expect("spawn");
    host.run(|_| false);
    assert_eq!(
        ended(&host, id),
        (Ending::Exit(0), expected::CPYTHON_STDOUT.into())
    );
    let template = host
        .spawn_with_files(image, &argv, files, true, false)
        .expect("spawn");
    while host.output(template) != Some(b"warm\n") {
        host.step();
    }
    assert!(host.freeze(template));
    let answer = expected::CPYTHON_STDOUT
        .strip_prefix("warm\n")
        .expect("warm first");
    let ids: Vec<u32> = (0..3)
        .map(|_| {
            host.spawn_from_template(template, true, true)
                .expect("fork")
        })
        .collect();
    host.run(|_| false);
    for id in ids {
        assert_eq!(ended(&host, id), (Ending::Exit(0), answer.into()));
    }
}

/// The CPython agent end to end (change `add-agent-workload`): it talks to
/// the host's fake model over HTTP through the network card for a few
/// turns, suspended and lazily resumed after each one with its connection
/// kept, then forks into branches at its decision point. Its transcript is
/// the one a Linux process prints (`cells/expected.rs`, checked on Linux by
/// `xtask`), and nothing stays open once the host is gone.
fn a_cpython_agent_talks_forks_and_survives_suspend() {
    if crate::cell::cpython_files().is_none() {
        arch::console::BootConsole.write_bytes(b"(CPython not built: not run) ");
        return;
    }
    if crate::wire::options().llm.is_none() {
        arch::console::BootConsole.write_bytes(b"(no network card and model server: not run) ");
        return;
    }
    let mut host = Host::new();
    let run = crate::cell::agent(
        &mut host,
        expected::AGENT_TURNS,
        Some(Resume::Lazy),
        b"fake",
        crate::cell::AgentOutput::Transcript,
    )
    .unwrap_or_else(|why| panic!("the agent did not finish: {why}"));
    assert_eq!(
        run.suspend_resume_ns.len(),
        expected::AGENT_TURNS,
        "suspended after every turn"
    );
    assert_eq!(
        alloc::string::String::from_utf8_lossy(&run.transcript),
        expected::AGENT_TRANSCRIPT
    );
    drop(host);
    assert_eq!(crate::cell::open_sockets(), 0);
    // The card's connections finish closing with the model server: the
    // kernel goes back to holding nothing for the agent.
    let mut host = Host::new();
    let end = crate::cell::now_ns() + 10_000_000_000;
    while crate::cell::wire_busy() && crate::cell::now_ns() < end {
        host.wire_address(0);
    }
    assert!(!crate::cell::wire_busy(), "the card's connections closed");
}
