//! Node.js (C.3), alone in the kernel built with Node.js embedded (change
//! `add-node-workload`), whose VM has the memory it needs.

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(console_output_works),
    ktest_case!(node_answers_as_on_linux),
    ktest_case!(a_node_cell_suspends_resumes_and_forks),
    ktest_case!(copies_draw_different_random),
];

/// Spawns Node.js on `/app/demo.js` (`expected::NODE_ARGV`) with the
/// script's arguments `args`.
fn spawn_node(host: &mut Host, files: &rhizo_fs::Fs, args: &[&str]) -> u32 {
    let mut argv: Vec<&[u8]> = expected::NODE_ARGV.iter().map(|a| a.as_bytes()).collect();
    argv.extend(args.iter().map(|a| a.as_bytes()));
    host.spawn_file(files.clone(), argv[0], &argv, &[], true, true)
        .unwrap_or_else(|e| panic!("cannot spawn Node.js: {e:?}"))
}

/// Node.js, the official build run unmodified with its JIT (spec
/// `workloads`, change `node-jit`), answers its tool call as on Linux (`cells/expected.rs`,
/// by the differential check), asks the host's fake model over HTTP with
/// `node:http` when the VM has a network card, and gives back every frame
/// and heap allocation once its file pages are dropped.
fn node_answers_as_on_linux() {
    let Some(files) = crate::cell::node_files() else {
        arch::console::BootConsole.write_bytes(b"(Node.js not packed: not run) ");
        return;
    };
    // The card's address first: its lease is kept.
    let mut host = Host::new();
    let model = crate::wire::options().llm;
    if model.is_some() {
        host.wire_address(10_000_000_000)
            .expect("the network card got an address");
    }
    let before = memory_balance();
    let id = spawn_node(&mut host, &files, &[]);
    host.run(|h| h.finished(id).is_some());
    assert_eq!(
        ended(&host, id),
        (Ending::Exit(0), expected::NODE_STDOUT.into())
    );
    drop(host);
    assert!(crate::cell::drop_images(), "file pages were kept");
    // Every frame is back; heap allocations too, but for the few the card's
    // own traffic (DHCP, ARP) holds at any moment.
    let after = memory_balance();
    assert_eq!(after.0, before.0, "every frame returned");
    assert!(after.1.abs_diff(before.1) <= 4, "{after:?} {before:?}");
    let Some((ip, port)) = model else {
        arch::console::BootConsole.write_bytes(b"(no model server: HTTP not run) ");
        return;
    };
    let mut host = Host::new();
    let url = alloc::format!("http://{}.{}.{}.{}:{port}", ip[0], ip[1], ip[2], ip[3]);
    let id = spawn_node(&mut host, &files, &[&url]);
    host.run(|h| h.finished(id).is_some());
    let want = alloc::format!("{}{}", expected::NODE_STDOUT, expected::NODE_MODEL_LINE);
    assert_eq!(ended(&host, id), (Ending::Exit(0), want));
    drop(host);
    assert_eq!(crate::cell::open_sockets(), 0, "its connection is closed");
}

/// A Node.js cell is plain data like any other (spec `cells`): suspended
/// after its warm-up, resumed from the snapshot's bytes, forked, and frozen
/// as a template, every copy finishes the answer.
fn a_node_cell_suspends_resumes_and_forks() {
    let Some(files) = crate::cell::node_files() else {
        arch::console::BootConsole.write_bytes(b"(Node.js not packed: not run) ");
        return;
    };
    let rest = expected::NODE_STDOUT
        .strip_prefix("warm\n")
        .expect("warm first");
    let mut host = Host::new();
    let warm = |host: &mut Host| {
        let id = spawn_node(host, &files, &["wait"]);
        host.run_to_pause(id, b"warm", Pause::Waits)
            .unwrap_or_else(|why| panic!("{why}: {:?}", ended(host, id)));
        id
    };
    let id = warm(&mut host);
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
    let template = warm(&mut host);
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
        assert_eq!(ended(&host, id), (Ending::Exit(0), rest.into()));
    }
}

/// Two copies of a Node.js template frozen after its warm-up each print a
/// `Math.random()` and a `crypto.randomUUID()` of their own: the script
/// replaces `Math.random` once it sees its copy generation changed, and
/// OpenSSL reseeds as `getpid` changed (spec `cell-hierarchy`, design D22
/// of `add-cell-hierarchy`).
fn copies_draw_different_random() {
    let Some(files) = crate::cell::node_files() else {
        arch::console::BootConsole.write_bytes(b"(Node.js not packed: not run) ");
        return;
    };
    let mut host = Host::new();
    let template = spawn_node(&mut host, &files, &["wait", "random"]);
    host.run_to_pause(template, b"warm", Pause::Waits)
        .unwrap_or_else(|why| panic!("{why}: {:?}", ended(&host, template)));
    assert!(host.freeze(template));
    let ids: Vec<u32> = (0..2)
        .map(|_| {
            host.spawn_from_template(template, true, false)
                .expect("copy")
        })
        .collect();
    host.run(|h| ids.iter().all(|&id| h.finished(id).is_some()));
    let answer = expected::NODE_STDOUT
        .strip_prefix("warm\n")
        .expect("warm first");
    let draws: Vec<alloc::string::String> = ids
        .iter()
        .map(|&id| {
            let (ending, out) = ended(&host, id);
            assert_eq!(ending, Ending::Exit(0), "{out}");
            let draw = out.strip_prefix(answer).expect("the answer first");
            assert_eq!(draw.split(' ').count(), 2, "{draw}");
            draw.into()
        })
        .collect();
    for (i, a) in draws.iter().enumerate() {
        for b in &draws[i + 1..] {
            for (x, y) in a.split(' ').zip(b.split(' ')) {
                assert_ne!(x, y, "two copies drew the same");
            }
        }
    }
}
