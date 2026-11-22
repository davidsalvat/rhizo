//! Cell handles (spec `cell-hierarchy`, task 2.5 of `add-cell-hierarchy`):
//! the observe stream, input, channels opened through the message right
//! and accepted on an inbox, control as authority over a cell, and the
//! exposure check on promotion. The cells are `cells/src/bin/handles.rs`.

use super::*;
use crate::cell::no_files;
use alloc::string::String;
use rhizo_cap::{Actor, Egress, Error as CapError, HOST, Limits, OnParentEnd, Rights, Spec};
use rhizo_cell::quota::Quota;
use rhizo_control::{Item, Message, State};

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[ktest_case!(cell_handles)];

const MIB: u64 = 1 << 20;

/// A `handles` cell in `mode`, its files `files`.
fn handles(host: &mut Host, files: rhizo_fs::Fs, args: &[&[u8]]) -> u32 {
    let mut argv = alloc::vec![&b"handles"[..]];
    argv.extend_from_slice(args);
    host.spawn_with_files(
        crate::cell::test_image("handles"),
        &argv,
        files,
        true,
        false,
    )
    .expect("spawn handles")
}

/// The output of ended cell `id`.
fn output(host: &Host, id: u32) -> String {
    ended(host, id).1
}

/// `bytes` split into their text and the messages of the control protocol
/// framed in them (what an observer copied of its observe stream).
fn decode(bytes: &[u8]) -> (String, Vec<Message>) {
    let mut decoder = rhizo_control::Decoder::default();
    let (mut text, mut messages) = (Vec::new(), Vec::new());
    for &b in bytes {
        match decoder.push(b) {
            Some(Item::Text(t)) => text.push(t),
            Some(Item::Frame(f)) => messages.push(Message::decode(&f).expect("a message")),
            Some(Item::Bad { .. }) => panic!("a bad frame"),
            None => {}
        }
    }
    (String::from_utf8_lossy(&text).into_owned(), messages)
}

/// The tests of task 2.5, one cell pair or group after the other.
fn cell_handles() {
    message_through_the_inbox();
    observe_and_input();
    revoked_handles_refuse();
    control_handles();
    promotion_keeps_exposure();
}

/// Spec `cell-hierarchy`, "Accepting from the inbox": a cell with the
/// message right on a server connects to its handle, the server accepts
/// on its inbox and names the client by its own handle's label; a cell
/// without the right gets `EACCES` for every use.
fn message_through_the_inbox() {
    let mut host = Host::new();
    let server = handles(&mut host, no_files(), &[b"serve"]);
    let client = handles(&mut host, no_files(), &[b"client", b"server"]);
    let stranger = handles(&mut host, no_files(), &[b"refused", b"server"]);
    host.grant(Actor::Host, client, server, Rights::MESSAGE, b"server")
        .expect("message right");
    host.grant(Actor::Host, server, client, Rights::OBSERVE, b"client")
        .expect("the server's handle on its client");
    host.grant(Actor::Host, stranger, server, Rights::NONE, b"server")
        .expect("a handle with no right");
    host.run(|_| false);
    assert_eq!(
        output(&host, server),
        "peer client\ngot ping\nclosed\n",
        "the server"
    );
    assert_eq!(output(&host, client), "reply pong\n", "the client");
    assert_eq!(
        output(&host, stranger),
        "connect -13 observe -13 input -13\n",
        "no right, nothing works"
    );
    assert!(host.tree().is_empty());
}

/// An observer reads a cell's output from when it opened its handle, then
/// how the cell ended, then end of file; a cell holding the input right
/// writes the cell's standard input, which wakes it.
fn observe_and_input() {
    let mut host = Host::new();
    let watcher = handles(&mut host, no_files(), &[b"observe", b"agent"]);
    let mut files = no_files();
    files.host_input().expect("standard input");
    let agent = handles(&mut host, files, &[b"echo"]);
    let writer = handles(&mut host, no_files(), &[b"input", b"agent", b"hello"]);
    host.grant(Actor::Host, watcher, agent, Rights::OBSERVE, b"agent")
        .expect("observe right");
    host.grant(Actor::Host, writer, agent, Rights::INPUT, b"agent")
        .expect("input right");
    host.run(|_| false);
    assert_eq!(output(&host, writer), "sent\n");
    assert_eq!(output(&host, agent), "echo hello\n", "input arrived");
    let raw = &host.finished(watcher).expect("the watcher ended").output;
    let (text, messages) = decode(raw);
    assert_eq!(text, "observing\nend\n");
    let seen: Vec<u8> = messages
        .iter()
        .filter_map(|m| match m {
            Message::Output { data, .. } => Some(data.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(seen, b"echo hello\n", "its output");
    assert!(
        matches!(
            messages.last(),
            Some(Message::Ended {
                state: State::Exited(0),
                ..
            })
        ),
        "then how it ended: {messages:?}"
    );
    assert!(host.tree().is_empty());
}

/// Spec `cell-hierarchy`, revocation: what was opened through a revoked
/// cell handle ends at once, and the handle refuses every use with
/// `EACCES`.
fn revoked_handles_refuse() {
    let mut host = Host::new();
    let target = spawn(&mut host, "idle", &[]);
    // Its only output comes before the watcher exists, so the stream the
    // watcher opens carries none, on any number of CPUs.
    host.run(|h| h.output(target).is_some_and(|o| o == b"idle\n"));
    let watcher = handles(&mut host, no_files(), &[b"observe", b"t"]);
    let g = host
        .grant(Actor::Host, watcher, target, Rights::OBSERVE, b"t")
        .expect("observe right");
    host.run(|h| h.output(watcher).is_some_and(|o| o == b"observing\n"));
    host.revoke(Actor::Host, g).expect("revoked");
    host.run(|h| h.finished(watcher).is_some());
    assert_eq!(output(&host, watcher), "observing\nend\n", "no last word");
    let late = handles(&mut host, no_files(), &[b"refused", b"t"]);
    assert_eq!(
        host.revoke(Actor::Host, g),
        Err(CapError::NotFound),
        "gone already"
    );
    let g = host
        .grant(Actor::Host, late, target, Rights::OBSERVE, b"t")
        .expect("observe right");
    host.revoke(Actor::Host, g).expect("revoked");
    host.run(|h| h.finished(late).is_some());
    assert_eq!(
        output(&host, late),
        "connect -13 observe -13 input -13\n",
        "a revoked handle"
    );
    assert!(host.kill(target));
    assert!(host.tree().is_empty());
}

/// Design D2: a handle with the control right lets its holder do to the
/// cell what an ancestor may (here kill and suspend it), and is never
/// granted to the cell it names or to a descendant of it.
fn control_handles() {
    let mut host = Host::new();
    let holder = spawn(&mut host, "idle", &[]);
    let target = spawn(&mut host, "idle", &[]);
    let other = spawn(&mut host, "idle", &[]);
    assert!(host.set_limits(
        target,
        Quota {
            memory: 64 * MIB,
            ..Quota::DEFAULT
        },
        4
    ));
    let below = spawn(&mut host, "idle", &[]);
    let spec = Spec {
        limits: Limits {
            memory: 8 * MIB,
            cpu_ns: u64::MAX,
            sockets: 16,
            cells: 0,
        },
        ..Spec::default()
    };
    host.adopt(below, target, spec, Actor::Host)
        .expect("placed");
    let by = Actor::Cell(holder);
    assert_eq!(host.kill_tree(other, by), Err(CapError::Denied));
    assert_eq!(
        host.grant(Actor::Host, target, target, Rights::CONTROL, b"me"),
        Err(CapError::Denied),
        "never on itself"
    );
    assert_eq!(
        host.grant(Actor::Host, below, target, Rights::CONTROL, b"up"),
        Err(CapError::Denied),
        "never up the tree"
    );
    host.grant(Actor::Host, holder, other, Rights::CONTROL, b"other")
        .expect("control right");
    host.grant(Actor::Host, holder, target, Rights::OBSERVE, b"target")
        .expect("observe right");
    assert_eq!(
        host.grant(Actor::Host, holder, target, Rights::CONTROL, b"target"),
        Err(CapError::Exists),
        "a label once per directory"
    );
    assert_eq!(
        host.suspend_tree(target, by).map(|s| s.len()),
        Err(CapError::Denied),
        "observing is not controlling"
    );
    host.run_until(crate::cell::now_ns() + 20_000_000, |_| false);
    let suspended = host
        .suspend_tree(other, by)
        .expect("suspended by its holder");
    assert_eq!(suspended.len(), 1);
    host.grant(Actor::Host, holder, target, Rights::CONTROL, b"t2")
        .expect("control right");
    host.kill_tree(target, by).expect("killed by its holder");
    assert_eq!(host.finished(below).map(|f| f.ending), Some(Ending::Killed));
    assert!(host.kill(holder));
    for (_, snapshot) in &suspended {
        host.drop_snapshot(snapshot);
    }
    assert!(host.tree().is_empty());
}

/// The promote vs exposure rule (design D8, D21): a cell's ancestors are
/// its trusted base, so its handles may let its data reach them; promoted,
/// it leaves them, and a promotion after which a former ancestor's wider
/// egress would reach it through a handle is refused, changing nothing, or,
/// at its parent's end, ends the cell, as one that cannot take its private
/// copies does.
fn promotion_keeps_exposure() {
    let mut host = Host::new();
    let root = spawn(&mut host, "idle", &[]);
    assert!(host.set_limits(
        root,
        Quota {
            memory: 32 * MIB,
            ..Quota::DEFAULT
        },
        8
    ));
    assert!(host.set_egress(root, Egress::Full));
    let narrow = |mib: u64, cells| Spec {
        limits: Limits {
            memory: mib * MIB,
            cpu_ns: u64::MAX,
            sockets: 16,
            cells,
        },
        egress: Egress::None,
        ceiling: Some(Egress::None),
        ..Spec::default()
    };
    let child = spawn(&mut host, "idle", &[]);
    host.adopt(child, root, narrow(4, 0), Actor::Host)
        .expect("placed");
    let g = host
        .grant(Actor::Host, root, child, Rights::OBSERVE, b"child")
        .expect("an ancestor observes");
    let before = host.tree().clone();
    assert_eq!(
        host.reparent(child, HOST, Actor::Host),
        Err(CapError::Denied),
        "promoted, its output would reach a wider egress"
    );
    assert_eq!(*host.tree(), before, "a refused promotion changes nothing");
    host.revoke(Actor::Host, g).expect("revoked");
    host.reparent(child, HOST, Actor::Host).expect("promoted");
    // At its parent's end.
    let parent = spawn(&mut host, "idle", &[]);
    host.adopt(parent, root, narrow(16, 2), Actor::Host)
        .expect("placed");
    let kept = spawn(&mut host, "idle", &[]);
    let promote = Spec {
        on_parent_end: OnParentEnd::Promote,
        ..narrow(4, 0)
    };
    host.adopt(kept, parent, promote, Actor::Host)
        .expect("placed");
    host.grant(Actor::Host, root, kept, Rights::OBSERVE, b"kept")
        .expect("an ancestor observes");
    host.run_until(crate::cell::now_ns() + 20_000_000, |_| false);
    assert!(host.kill(parent));
    assert_eq!(
        host.finished(kept).map(|f| f.ending),
        Some(Ending::Killed),
        "it would have left its ceiling: it ends"
    );
    for cell in [root, child] {
        assert!(host.kill(cell));
    }
    assert!(host.tree().is_empty());
}
