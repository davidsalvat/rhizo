//! Sockets and pipes (spec `networking`).

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(sockets_and_pipes_behave_as_on_linux),
    ktest_case!(cells_talk_over_loopback),
    ktest_case!(a_waiting_cell_uses_no_cpu),
    ktest_case!(only_the_cells_waiting_on_a_socket_wake),
    ktest_case!(latencies_count_from_the_first_idle_moment),
    ktest_case!(sockets_across_suspend_and_resume),
    ktest_case!(a_discarded_snapshot_closes_its_connections),
    ktest_case!(loopback_connection_across_suspend_of_one_end),
    ktest_case!(a_connection_to_the_host_survives_suspend),
    ktest_case!(connections_of_a_template_reset_in_its_cells),
    ktest_case!(a_blocked_cell_survives_suspend),
    ktest_case!(forked_servers_share_their_listener),
    ktest_case!(pipes_and_sockets_of_a_fork),
    ktest_case!(a_cell_reaches_the_host, 31),
    ktest_case!(a_silent_connection_outlasts_the_timeout_and_reconnects_at_once),
];

/// Pipes, event counters, `poll`, `epoll`, `select` and sockets through a
/// real libc print what they print on Linux; every frame comes back.
fn sockets_and_pipes_behave_as_on_linux() {
    assert_no_leak("libc_net");
    for name in ["loopback", "piper", "netstate"] {
        run_expected(name);
    }
    assert_eq!(crate::cell::open_sockets(), 0, "every socket closed");
}

/// A server cell and a client cell exchange messages over TCP on 127.0.0.1.
fn cells_talk_over_loopback() {
    let mut host = Host::new();
    let server = spawn(&mut host, "loopback", &[b"serve", b"7300", b"100"]);
    let client = spawn(&mut host, "loopback", &[b"ping", b"7300", b"100"]);
    host.run(|_| false);
    assert_eq!(ended(&host, server), (Ending::Exit(0), "served\n".into()));
    assert_eq!(ended(&host, client), (Ending::Exit(0), "pinged\n".into()));
    assert_eq!(crate::cell::open_sockets(), 0);
}

/// A server waiting in `accept` does not run while another cell works; it
/// runs once a client connects.
fn a_waiting_cell_uses_no_cpu() {
    let mut host = Host::new();
    let server = spawn(&mut host, "loopback", &[b"serve", b"7301", b"1"]);
    host.step(); // it reaches `accept` and waits
    let chatter = spawn(&mut host, "chatter", &[]);
    host.run(|h| h.finished(chatter).is_some());
    assert!(host.finished(server).is_none(), "the server still waits");
    assert_eq!(host.waiting(), 1, "and it is not runnable");
    let client = spawn(&mut host, "loopback", &[b"ping", b"7301", b"1"]);
    host.run(|_| false);
    assert_eq!(ended(&host, server), (Ending::Exit(0), "served\n".into()));
    assert_eq!(ended(&host, client).0, Ending::Exit(0));
}

/// A socket change wakes only the cells waiting on that socket (design D1
/// of `add-socket-wait-queues`): a server waiting for a client that never
/// comes stays asleep while two other cells talk, and no wake-up finds
/// nothing to do.
fn only_the_cells_waiting_on_a_socket_wake() {
    use crate::prof::{self, Count};
    let mut host = Host::new();
    let idle = spawn(&mut host, "loopback", &[b"serve", b"7310", b"1"]);
    step_until_waiting(&mut host); // it reaches `accept` and waits
    prof::start();
    let server = spawn(&mut host, "loopback", &[b"serve", b"7311", b"50"]);
    let client = spawn(&mut host, "loopback", &[b"ping", b"7311", b"50"]);
    host.run(|h| h.finished(server).is_some() && h.finished(client).is_some());
    let report = prof::stop();
    assert_eq!(ended(&host, server), (Ending::Exit(0), "served\n".into()));
    assert_eq!(ended(&host, client), (Ending::Exit(0), "pinged\n".into()));
    assert_eq!(report.count(Count::IoWasted), 0, "no wake-up found nothing");
    assert!(host.finished(idle).is_none());
    assert_eq!(host.waiting(), 1, "the idle server was never woken");
    assert!(host.kill(idle));
    assert_eq!(crate::cell::open_sockets(), 0);
}

/// After `prof::settle`, latencies count from the first moment no cell runs
/// or waits for a CPU (design D6 of `add-socket-wait-queues`): a cell that
/// yields 2,000 times once counting starts is not counted, the deadlines of
/// cells started after it, which pass once they all sleep, are. Without
/// it, every latency counts.
fn latencies_count_from_the_first_idle_moment() {
    use crate::prof;
    const SLEEPERS: u64 = 4;
    for settle in [false, true] {
        let mut host = Host::new();
        prof::start();
        if settle {
            prof::settle();
        }
        let yielder = spawn(&mut host, "yielder", &[]);
        host.run(|h| h.finished(yielder).is_some());
        let sleepers: Vec<u32> = (0..SLEEPERS)
            .map(|_| spawn(&mut host, "sleeper", &[b"250"]))
            .collect();
        host.run(|h| sleepers.iter().all(|&id| h.finished(id).is_some()));
        let report = prof::stop();
        for &id in sleepers.iter().chain([&yielder]) {
            assert_eq!(ended(&host, id).0, Ending::Exit(0));
        }
        assert_eq!(report.wake.count(), SLEEPERS, "each deadline counts");
        if settle {
            assert!(report.settled_ns > 0, "a moment came when no cell ran");
            assert_eq!(report.sched.count(), SLEEPERS, "only the woken cells");
        } else {
            assert_eq!(report.settled_ns, 0);
            assert!(report.sched.count() > SLEEPERS + 1000, "the yields count");
        }
    }
}

/// Runs `netstate` to its `ready` line.
fn netstate_ready(host: &mut Host) -> u32 {
    let id = spawn(host, "netstate", &[]);
    while host.output(id) != Some(b"ready\n") {
        host.step();
    }
    id
}

/// Suspended at `ready` and resumed from its snapshot's bytes, `netstate`
/// finds its pipe's bytes, its listener and its connection (parked in the
/// kernel meanwhile); a copy of the same snapshot resumed after it finds
/// the connection reset (spec `networking`).
fn sockets_across_suspend_and_resume() {
    let mut host = Host::new();
    let id = netstate_ready(&mut host);
    let snapshot = host.suspend(id).expect("live");
    assert_eq!(
        crate::cell::open_sockets(),
        3,
        "a suspended cell's listener and connection's two ends, parked"
    );
    let bytes = snapshot.export(&DirectMap, crate::arch::mem::crc32c);
    let copy = Snapshot::from_bytes(
        bytes.clone(),
        crate::arch::user::MACHINE,
        crate::arch::mem::crc32c,
    )
    .expect("valid");
    let snapshot =
        Snapshot::from_bytes(bytes, crate::arch::user::MACHINE, crate::arch::mem::crc32c)
            .expect("valid");
    let id = host
        .resume(snapshot, Resume::Lazy, true, true)
        .expect("resume");
    host.run(|_| false);
    assert_eq!(
        ended(&host, id),
        (
            Ending::Exit(0),
            "pipe kept\nconnection open\nlistener ok\n".into()
        )
    );
    assert_eq!(crate::cell::open_sockets(), 0);
    let id = host
        .resume(copy, Resume::Eager, true, true)
        .expect("resume");
    host.run(|_| false);
    assert_eq!(
        ended(&host, id),
        (
            Ending::Exit(0),
            "pipe kept\nconnection reset\nlistener ok\n".into()
        )
    );
    assert_eq!(crate::cell::open_sockets(), 0);
}

/// A snapshot dropped without a resume closes its parked connections.
fn a_discarded_snapshot_closes_its_connections() {
    let mut host = Host::new();
    let id = netstate_ready(&mut host);
    let snapshot = host.suspend(id).expect("live");
    assert_eq!(crate::cell::open_sockets(), 3, "parked");
    crate::cell::close_parked(&snapshot);
    assert_eq!(crate::cell::open_sockets(), 0, "closed");
}

/// Runs `loopback serve PORT N` and `loopback ping PORT N` (with `addr`, or
/// against the server), suspending cell `moving` (0: server, 1: client)
/// after every scheduling step and resuming it at once, after `pause` when
/// given (a cell run meanwhile); returns the moving cell's whole output and
/// the number of resumes.
fn suspended_every_step(
    host: &mut Host,
    port: &[u8],
    n: &[u8],
    moving: usize,
    addr: Option<&[u8]>,
    pause: Option<&str>,
) -> (Vec<u8>, usize) {
    let mut ids = alloc::vec::Vec::new();
    if addr.is_none() {
        ids.push(spawn(host, "loopback", &[b"serve", port, n]));
    }
    let mut ping = alloc::vec![b"ping".as_slice(), port, n];
    ping.extend(addr);
    ids.push(spawn(host, "loopback", &ping));
    let mut output = Vec::new();
    let mut resumes = 0;
    loop {
        host.step();
        if let Some(done) = host.finished(ids[moving]) {
            assert_eq!(done.ending, Ending::Exit(0));
            output.extend_from_slice(&done.output);
            break;
        }
        let snapshot = host.suspend(ids[moving]).expect("live");
        output.extend_from_slice(&host.finished(ids[moving]).expect("suspended").output);
        if let Some(cell) = pause {
            let other = spawn(host, cell, &[]);
            host.run(|h| h.finished(other).is_some());
        }
        ids[moving] = host
            .resume(snapshot, Resume::Lazy, true, true)
            .expect("resume");
        resumes += 1;
    }
    host.run(|_| false);
    for (i, &id) in ids.iter().enumerate() {
        if i != moving {
            assert_eq!(ended(host, id).0, Ending::Exit(0));
        }
    }
    (output, resumes)
}

/// A loopback TCP connection between two cells keeps working while either
/// end is suspended and resumed after every scheduling step.
fn loopback_connection_across_suspend_of_one_end() {
    for (moving, port, said) in [(0, b"7304", "served\n"), (1, b"7305", "pinged\n")] {
        let mut host = Host::new();
        let (output, resumes) = suspended_every_step(&mut host, port, b"20", moving, None, None);
        assert!(resumes >= 20, "suspended {resumes} times");
        assert_eq!(alloc::string::String::from_utf8_lossy(&output), said);
        drop(host);
        assert_eq!(crate::cell::open_sockets(), 0);
    }
}

/// A connection to the host's echo server through the network card keeps
/// working while its cell is suspended after every step, with the card
/// polled meanwhile (a `sleeper` runs); not run without a card.
fn a_connection_to_the_host_survives_suspend() {
    let Some((ip, port)) = crate::wire::options().echo else {
        arch::console::BootConsole.write_bytes(b"(no network card and echo server: not run) ");
        return;
    };
    let mut host = Host::new();
    assert!(
        host.wire_address(10_000_000_000).is_some(),
        "the card gets an address"
    );
    let ip = alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    let port = alloc::format!("{port}");
    let (output, resumes) = suspended_every_step(
        &mut host,
        port.as_bytes(),
        b"10",
        0,
        Some(ip.as_bytes()),
        Some("sleeper"),
    );
    assert!(resumes >= 10, "suspended {resumes} times");
    assert_eq!(alloc::string::String::from_utf8_lossy(&output), "pinged\n");
    drop(host);
    assert_eq!(crate::cell::open_sockets(), 0);
}

/// A server suspended while it waits in `accept` waits there again after
/// its resume, and serves the client that comes.
/// Steps `host` until one cell waits on its descriptors (a server in
/// `accept`): a slice may end, preempted, before it gets there (under
/// emulation a tick can be due as the slice starts).
fn step_until_waiting(host: &mut Host) {
    for _ in 0..100 {
        if host.waiting() == 1 {
            return;
        }
        host.step();
    }
    assert_eq!(host.waiting(), 1, "the server waits in accept");
}

fn a_blocked_cell_survives_suspend() {
    let mut host = Host::new();
    let server = spawn(&mut host, "loopback", &[b"serve", b"7302", b"1"]);
    step_until_waiting(&mut host);
    assert_eq!(host.waiting(), 1);
    let snapshot = host.suspend(server).expect("live");
    let server = host
        .resume(snapshot, Resume::Eager, true, true)
        .expect("resume");
    let client = spawn(&mut host, "loopback", &[b"ping", b"7302", b"1"]);
    host.run(|_| false);
    assert_eq!(ended(&host, server), (Ending::Exit(0), "served\n".into()));
    assert_eq!(ended(&host, client), (Ending::Exit(0), "pinged\n".into()));
}

/// A server frozen while it waits in `accept`: two cells spawned from it
/// share its listener and each serves one of two clients.
fn forked_servers_share_their_listener() {
    let mut host = Host::new();
    let template = spawn(&mut host, "loopback", &[b"serve", b"7303", b"1"]);
    step_until_waiting(&mut host);
    assert!(host.freeze(template));
    let servers: Vec<u32> = (0..2)
        .map(|_| {
            host.spawn_from_template(template, true, true)
                .expect("fork")
        })
        .collect();
    let clients: Vec<u32> = (0..2)
        .map(|_| spawn(&mut host, "loopback", &[b"ping", b"7303", b"1"]))
        .collect();
    host.run(|_| false);
    for id in servers {
        assert_eq!(ended(&host, id), (Ending::Exit(0), "served\n".into()));
    }
    for id in clients {
        assert_eq!(ended(&host, id), (Ending::Exit(0), "pinged\n".into()));
    }
    drop(host);
    assert_eq!(
        crate::cell::open_sockets(),
        0,
        "the template's listener closed"
    );
}

/// Forked at `ready`, `netstate` and its child both read the pipe's bytes;
/// the child's connection is reset, the parent's is not, and both accept
/// on the listener they share.
fn pipes_and_sockets_of_a_fork() {
    let mut host = Host::new();
    let parent = netstate_ready(&mut host);
    let child = host.fork(parent, 1).expect("fork")[0];
    host.run(|_| false);
    assert_eq!(
        ended(&host, parent),
        (
            Ending::Exit(0),
            "ready\npipe kept\nconnection open\nlistener ok\n".into()
        )
    );
    assert_eq!(
        ended(&host, child),
        (
            Ending::Exit(0),
            "pipe kept\nconnection reset\nlistener ok\n".into()
        )
    );
    assert_eq!(crate::cell::open_sockets(), 0);
}

/// Frozen at `ready`, `netstate` keeps its connection as a template; a cell
/// spawned from it finds the connection reset and accepts on the listener
/// it shares with the template.
fn connections_of_a_template_reset_in_its_cells() {
    let mut host = Host::new();
    let template = netstate_ready(&mut host);
    assert!(host.freeze(template));
    let id = host
        .spawn_from_template(template, true, true)
        .expect("spawn");
    host.run(|_| false);
    assert_eq!(
        ended(&host, id),
        (
            Ending::Exit(0),
            "pipe kept\nconnection reset\nlistener ok\n".into()
        )
    );
    assert_eq!(crate::cell::open_sockets(), 3, "the template's");
    drop(host);
    assert_eq!(crate::cell::open_sockets(), 0);
}

/// A cell reaches a TCP echo server on the host through the network card
/// (`echo=` on the command line, given by `xtask` where the VMM has a
/// card); without one the test says it did not run.
fn a_cell_reaches_the_host() {
    let Some((ip, port)) = crate::wire::options().echo else {
        arch::console::BootConsole.write_bytes(b"(no network card and echo server: not run) ");
        return;
    };
    let mut host = Host::new();
    assert!(
        host.wire_address(10_000_000_000).is_some(),
        "the card gets an address"
    );
    let ip = alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    let port = alloc::format!("{port}");
    let id = spawn(
        &mut host,
        "loopback",
        &[b"ping", port.as_bytes(), b"20", ip.as_bytes()],
    );
    host.run(|_| false);
    assert_eq!(ended(&host, id), (Ending::Exit(0), "pinged\n".into()));
}

/// A connection through the network card that stays silent for three
/// times the card's timeout (shortened to 500 ms here, 20 s otherwise)
/// still carries the next echo, and a new connection after it closes is
/// open and echoing within a second (spec `networking`, change
/// `fix-tcp-long-wait`); not run without a card.
fn a_silent_connection_outlasts_the_timeout_and_reconnects_at_once() {
    let Some((ip, port)) = crate::wire::options().echo else {
        arch::console::BootConsole.write_bytes(b"(no network card and echo server: not run) ");
        return;
    };
    let mut host = Host::new();
    assert!(
        host.wire_address(10_000_000_000).is_some(),
        "the card gets an address"
    );
    crate::cell::set_wire_timeout(500_000_000);
    let ip = alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    let port = alloc::format!("{port}");
    let id = spawn(
        &mut host,
        "quiet",
        &[port.as_bytes(), b"1500", ip.as_bytes()],
    );
    host.run(|_| false);
    crate::cell::set_wire_timeout(20_000_000_000);
    assert_eq!(ended(&host, id), (Ending::Exit(0), "quiet ok\n".into()));
}
