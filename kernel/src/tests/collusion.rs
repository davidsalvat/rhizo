//! Hostile collusion between domains (design D0 and D12 of
//! `add-cell-hierarchy`, task 1.12): colluders that try to signal each
//! other through the kernel, a fork bomb inside a subtree, and a snapshot
//! file forged to claim authority. The colluders are `cells/src/bin/colluder.rs`.

use super::*;
use alloc::string::String;
use rhizo_cap::{Actor, Egress, Error as TreeError, HOST, Limits, Spec};
use rhizo_cell::quota::Quota;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(differential_colluders, 60),
    ktest_case!(colluders_on_loopback),
    ktest_case!(fork_bomb_in_a_subtree),
    ktest_case!(forged_snapshot_authority),
];

const MIB: u64 = 1 << 20;

/// A reserved root's quota: `mib` MiB and `sockets` sockets.
fn reserved(mib: u64, sockets: u32) -> Quota {
    Quota {
        memory: mib * MIB,
        sockets,
        ..Quota::DEFAULT
    }
}

/// A child's limits: `mib` MiB, `sockets` sockets, `cells` descendants.
fn child(mib: u64, sockets: u32, cells: u32) -> Spec {
    Spec {
        limits: Limits {
            memory: mib * MIB,
            cpu_ns: u64::MAX,
            sockets,
            cells,
        },
        ..Spec::default()
    }
}

/// Steps the host until cell `id` has printed `said`, at least.
fn until_said(host: &mut Host, id: u32, said: &[u8]) {
    while !host.output(id).is_some_and(|o| o.starts_with(said)) {
        assert!(host.finished(id).is_none(), "{:?}", ended(host, id));
        host.step();
    }
}

/// How much the cells `ids` printed in all.
fn said(host: &mut Host, ids: &[u32]) -> usize {
    ids.iter()
        .map(|&id| host.output(id).map_or(0, <[u8]>::len))
        .sum()
}

/// The receiver of `differential_colluders` with its two children: its
/// probe run once the host wakes it, with a sender's domain working beside
/// it when `beside`. Returns the outputs of the receiver and its children.
fn receive(host: &mut Host, outside: Option<&str>, beside: bool) -> Vec<String> {
    let mut argv: Vec<&[u8]> = alloc::vec![b"probe"];
    argv.extend(outside.map(str::as_bytes));
    let receiver = spawn(host, "colluder", &argv);
    assert!(host.set_limits(receiver, reserved(12, 16), 2));
    assert!(host.set_egress(receiver, crate::wire::echo_egress()));
    // Children it holds: they take their numbers in its domain.
    let children: Vec<u32> = (0..2)
        .map(|_| {
            let c = spawn(host, "observer", &[b"ids"]);
            host.adopt(c, receiver, child(2, 0, 0), Actor::Host)
                .expect("placed");
            c
        })
        .collect();
    until_said(host, receiver, b"ready\n");
    // Its own children's lives show in its own figures (their limits are
    // carved from its own): they end before it probes.
    host.run(|h| children.iter().all(|&c| h.finished(c).is_some()));
    let mut senders = Vec::new();
    let mut dots = 0;
    if beside {
        // A reserved root of the same size with two children, all sending,
        // and copies of it, reserved roots of its domain, until the kernel
        // refuses one (at most two).
        let mut argv: Vec<&[u8]> = alloc::vec![b"send"];
        argv.extend(outside.map(str::as_bytes));
        let sender = spawn(host, "colluder", &argv);
        assert!(host.set_limits(sender, reserved(12, 16), 2));
        assert!(host.set_egress(sender, crate::wire::echo_egress()));
        assert_ne!(host.domain(sender), host.domain(receiver));
        for _ in 0..2 {
            let c = spawn(host, "colluder", &argv);
            host.adopt(c, sender, child(4, 4, 0), Actor::Host)
                .expect("placed");
            senders.push(c);
        }
        until_said(host, sender, b"sending\n");
        for _ in 0..2 {
            match host.fork(sender, 1) {
                Some(copies) => senders.extend(copies),
                None => break,
            }
        }
        senders.push(sender);
        dots = said(host, &senders);
    }
    assert!(host.wake(receiver));
    host.run(|h| {
        h.finished(receiver).is_some() && children.iter().all(|&c| h.finished(c).is_some())
    });
    let mut outputs = Vec::new();
    for id in core::iter::once(receiver).chain(children) {
        let (ending, out) = ended(host, id);
        assert_eq!(ending, Ending::Exit(0), "{out}");
        outputs.push(out);
    }
    if beside {
        // The senders went on working while the receiver probed.
        let after = said(host, &senders);
        assert!(
            after > dots,
            "no sender made a round while the receiver probed"
        );
        for id in senders.into_iter().rev() {
            let _ = host.kill_tree(id, Actor::Host);
        }
        host.run(|_| false);
    }
    assert!(host.tree().is_empty(), "every cell ended");
    outputs
}

/// Hostile (spec `isolation`, "Differential colluders"): a receiver in one
/// reserved domain runs a fixed probe (memory and a file to its limit,
/// sockets to its limit, loopback binds and connections on fixed ports,
/// ephemeral ports, a connection outside, file pages, `sysinfo`,
/// `getpid`, threads, and two children numbered in its domain), once alone
/// and once while a sender's domain does each of these as hard as it can,
/// copies of the sender included. Apart from times, every line it and its
/// children print is the same both times, and every frame comes back.
fn differential_colluders() {
    let mut host = Host::new();
    let echo = crate::wire::options().echo;
    let outside = echo
        .filter(|_| host.wire_address(10_000_000_000).is_some())
        .map(super::net::endpoint);
    let alone = receive(&mut host, outside.as_deref(), false);
    let probe = &alone[0];
    for line in [
        "memory mib=",
        "loopback listen=0 again=98 connect=0 echo=ok",
        "pages sum=",
    ] {
        assert!(probe.contains(line), "{probe}");
    }
    assert_eq!(
        probe.contains("outside echo=ok"),
        outside.is_some(),
        "{probe}"
    );
    drop(host);
    // Beside a sender, twice. Every frame comes back (the count of heap
    // allocations moves by a few between identical rounds, both ways,
    // with what the kernel keeps of executables and the card's stack, so
    // the heap is counted in pages).
    let before = memory_balance().0;
    for _ in 0..2 {
        let mut host = Host::new();
        let beside = receive(&mut host, outside.as_deref(), true);
        assert_eq!(alone, beside, "the receiver saw the sender");
    }
    assert_eq!(crate::cell::open_sockets(), 0);
    assert_eq!(memory_balance().0, before, "every frame came back");
}

/// Hostile (spec `networking`, "Echo between two cells", design D5): a
/// colluder listens on TCP and UDP port 7500 of every address and on a Unix
/// socket; a colluder of another domain, with egress `full`, knocks on
/// 127.0.0.1:7500, the Unix socket and, where the kernel has a card, the
/// card's own address. It reaches nothing: `ECONNREFUSED` on loopback,
/// no such name, a connection to the card's address still being made
/// after 200 ms, its datagrams sent but delivered to no one; it binds port
/// 7500 itself, and the listener heard nothing.
fn colluders_on_loopback() {
    let mut host = Host::new();
    let card = host.wire_address(10_000_000_000);
    let listener = spawn(&mut host, "colluder", &[b"listen", b"7500"]);
    until_said(&mut host, listener, b"listening\n");
    let card = card.map(|ip| alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]));
    let mut argv: Vec<&[u8]> = alloc::vec![b"knock", b"7500"];
    argv.extend(card.as_deref().map(str::as_bytes));
    let knocker = spawn(&mut host, "colluder", &argv);
    assert!(host.set_egress(knocker, Egress::Full));
    assert_ne!(host.domain(knocker), host.domain(listener));
    host.run(|h| h.finished(knocker).is_some());
    let want = if card.is_some() {
        "RSNWS\nbound\n"
    } else {
        "RSN\nbound\n"
    };
    assert_eq!(ended(&host, knocker), (Ending::Exit(0), want.into()));
    assert!(host.wake(listener));
    host.run(|h| h.finished(listener).is_some());
    assert_eq!(
        ended(&host, listener),
        (Ending::Exit(0), "listening\nheard nothing\n".into())
    );
    drop(host);
    assert_eq!(crate::cell::open_sockets(), 0);
}

/// Hostile (spec `cell-hierarchy`, "Fork bomb"): under a root with a limit
/// of 8 descendants and 64 MiB, the host forks every cell of the subtree
/// and places new cells under each, round after round, as children trying
/// to make children without end would. The subtree never holds more than
/// 8 descendants, the rest are refused for the budget, a cell of another
/// domain runs to its end meanwhile, and everything comes back when the
/// subtree ends.
fn fork_bomb_in_a_subtree() {
    let before = memory_balance();
    let mut host = Host::new();
    let root = spawn(&mut host, "spin", &[]);
    assert!(host.set_limits(root, reserved(64, 64), 8));
    let other = spawn(&mut host, "chatter", &[b"c"]);
    let (mut made, mut refused) = (0, 0);
    for _ in 0..4 {
        for cell in host.tree().subtree(root).unwrap_or_default() {
            match host.fork(cell, 1) {
                Some(_) if cell != root => made += 1,
                Some(copies) => {
                    // A copy of the root is a root of its own: not in the
                    // subtree, and not the bomb's.
                    for c in copies {
                        assert!(host.kill(c));
                    }
                }
                None => refused += 1,
            }
            let new = spawn(&mut host, "spin", &[]);
            match host.adopt(new, cell, child(4, 2, 2), Actor::Host) {
                Ok(()) => made += 1,
                Err(e) => {
                    assert_eq!(e, TreeError::Budget);
                    assert!(host.kill(new));
                    refused += 1;
                }
            }
            let held = host.tree().subtree(root).map_or(0, |s| s.len());
            assert!(held <= 9, "{held} cells in the subtree");
        }
        host.run_until(crate::cell::now_ns() + 20_000_000, |_| false);
    }
    assert!(
        made > 0 && made <= 8 && refused > 0,
        "{made} made, {refused} refused"
    );
    host.run(|h| h.finished(other).is_some());
    assert_eq!(ended(&host, other), (Ending::Exit(0), "c\nc\nc\n".into()));
    host.kill_tree(root, Actor::Host).expect("ended");
    host.run(|_| false);
    assert!(host.tree().is_empty());
    drop(host);
    assert_eq!(memory_balance(), before, "every frame came back");
}

/// Hostile (spec `snapshots`, "Forged authority"): a cell's snapshot
/// forged to claim egress `full`, a 4 GiB limit, every CPU and unlimited
/// descendants, sealed again, is loaded as the control kernel loads a file
/// and placed under a parent with egress `none`: it and its copy have
/// egress `none`, the limits the host gave them within the parent's
/// budget, no CPU set, and their connection outside is denied. Resumed as
/// a root instead, the 4 GiB it claims are refused (the kernel cannot set
/// them aside), and with a claim the kernel can hold it still gets egress
/// `none` and no CPU set.
fn forged_snapshot_authority() {
    let target = crate::wire::options()
        .echo
        .map_or_else(|| String::from("192.0.2.1:9"), super::net::endpoint);
    let mut host = Host::new();
    let source = spawn(&mut host, "colluder", &[b"dial", target.as_bytes()]);
    until_said(&mut host, source, b"ready\n");
    let mut snapshot = host.suspend(source).expect("suspend");
    let forged = |snapshot: &mut Snapshot, memory: u64| {
        snapshot.set_tree(&Spec {
            limits: Limits {
                memory,
                cpu_ns: u64::MAX,
                sockets: u32::MAX,
                cells: u32::MAX,
            },
            egress: Egress::Full,
            ceiling: Some(Egress::Full),
            cpus: u64::MAX,
            ..Spec::default()
        });
        snapshot.export(&DirectMap, crate::arch::mem::crc32c)
    };
    let (huge, small) = (
        forged(&mut snapshot, 4 << 30),
        forged(&mut snapshot, 8 * MIB),
    );
    host.drop_snapshot(&snapshot);
    drop(snapshot);
    let load = |bytes: &Vec<u8>, placed: bool| {
        let mut s =
            Snapshot::from_bytes(bytes.clone(), arch::user::MACHINE, crate::arch::mem::crc32c)
                .expect("sealed again: valid");
        s.cap_quota(Quota::DEFAULT); // as the control kernel does
        if placed {
            crate::cell::to_place(&mut s);
        }
        s
    };
    let parent = spawn(&mut host, "idle", &[]);
    assert!(host.set_limits(parent, reserved(16, 8), 2));
    let template = host
        .load_template(load(&huge, true))
        .unwrap_or_else(|_| panic!("not loaded"));
    host.place(template, parent, child(4, 4, 0), 0)
        .expect("placed under the parent");
    let copy = host
        .spawn_from_template(template, true, true)
        .expect("a copy");
    assert!(
        host.resume(load(&huge, false), Resume::Eager, true, true)
            .is_err(),
        "4 GiB set aside for bytes"
    );
    let resumed = host
        .resume(load(&small, false), Resume::Eager, true, true)
        .unwrap_or_else(|_| panic!("not resumed as a root"));
    for (cell, under) in [(template, parent), (copy, parent), (resumed, HOST)] {
        let n = host.tree().get(cell).expect("in the tree");
        assert_eq!(n.parent(), under);
        assert_eq!(*n.egress(), Egress::None, "cell {cell}");
        assert_eq!(n.spec().ceiling, Some(Egress::None), "cell {cell}");
        assert_eq!(n.spec().cpus, 0, "cell {cell}");
        let most = if under == HOST { 8 * MIB } else { 4 * MIB };
        assert!(n.limits().memory <= most, "cell {cell}: {:?}", n.limits());
    }
    for cell in [copy, resumed] {
        assert!(host.wake(cell));
    }
    host.run(|h| h.finished(copy).is_some() && h.finished(resumed).is_some());
    for cell in [copy, resumed] {
        let (ending, out) = ended(&host, cell);
        assert_eq!(ending, Ending::Exit(0), "{out}");
        assert!(out.ends_with("D\n"), "cell {cell} reached out: {out}");
    }
    // The parent's end takes the template with it.
    host.kill_tree(parent, Actor::Host).expect("ended");
    host.run(|_| false);
    assert!(host.tree().is_empty());
}
