//! Sockets and pipes (spec `networking`).

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(sockets_and_pipes_behave_as_on_linux),
    ktest_case!(cells_talk_over_loopback),
    ktest_case!(separate_loopback_networks),
    ktest_case!(a_waiting_cell_uses_no_cpu),
    ktest_case!(only_the_cells_waiting_on_a_socket_wake),
    ktest_case!(latencies_count_from_the_first_idle_moment),
    ktest_case!(sockets_across_suspend_and_resume),
    ktest_case!(a_discarded_snapshot_closes_its_connections),
    ktest_case!(loopback_connection_across_suspend_of_one_end),
    ktest_case!(a_connection_to_the_host_survives_suspend),
    ktest_case!(connections_of_a_template_reset_in_its_cells),
    ktest_case!(a_blocked_cell_survives_suspend),
    ktest_case!(forked_servers_in_their_network),
    ktest_case!(pipes_and_sockets_of_a_fork),
    ktest_case!(a_cell_reaches_the_host, 31),
    ktest_case!(a_silent_connection_outlasts_the_timeout_and_reconnects_at_once),
    ktest_case!(egress_levels_against_the_echo_server),
    ktest_case!(ports_of_other_domains_do_not_show),
];

/// `ADDR:PORT` of `at`.
pub(super) fn endpoint((ip, port): ([u8; 4], u16)) -> alloc::string::String {
    alloc::format!("{}.{}.{}.{}:{port}", ip[0], ip[1], ip[2], ip[3])
}

/// Each egress level against the host (spec `networking`, "Each level",
/// "Default" and "Host services behind the gateway"; design D7 of
/// `add-cell-hierarchy`): cells with no policy, `model`, a rule for the
/// echo server and `full` connect to the model endpoint, the echo server
/// and another port of the host. Each connects exactly where its policy
/// allows, and the host's other port, behind its gateway, is in the deny
/// floor for all of them, `full` included. Not run without a card, an echo
/// server and a model server.
fn egress_levels_against_the_echo_server() {
    let opts = crate::wire::options();
    let (Some(echo), Some(model)) = (opts.echo, opts.llm) else {
        arch::console::BootConsole
            .write_bytes(b"(no network card, echo and model servers: not run) ");
        return;
    };
    let mut host = Host::new();
    assert!(
        host.wire_address(10_000_000_000).is_some(),
        "the card gets an address"
    );
    let targets = [endpoint(model), endpoint(echo), endpoint((echo.0, 1))];
    let argv: Vec<&[u8]> = targets.iter().map(|t| t.as_bytes()).collect();
    for (egress, want) in [
        (None, "DDD\n"),
        (Some(crate::wire::model_egress()), "CDD\n"),
        (Some(crate::wire::echo_egress()), "DCD\n"),
        (Some(rhizo_net::Egress::Full), "CCD\n"),
    ] {
        // Records of earlier tests out of the way: nothing empties the ring
        // in a test build.
        while crate::audit::take().is_some() {}
        let id = spawn(&mut host, "reach", &argv);
        if let Some(egress) = egress {
            assert!(host.set_egress(id, egress));
        }
        host.run(|h| h.finished(id).is_some());
        assert_eq!(ended(&host, id), (Ending::Exit(0), want.into()));
        // Each refusal is audited, naming the cell and the destination;
        // those of one second fold into one more record, which leaves once
        // the second is over (spec `networking`, "Egress denials are
        // audited").
        let pause = spawn(&mut host, "sleeper", &[b"1100"]);
        host.run(|h| h.finished(pause).is_some());
        let mut denied = Vec::new();
        while let Some(r) = crate::audit::take() {
            if r.kind == crate::audit::Kind::Egress && r.cell == id {
                denied.push((r.count, r.detail));
            }
        }
        let refusals = want.bytes().filter(|&b| b == b'D').count() as u32;
        let first = alloc::format!("tcp {}", targets[want.find('D').unwrap_or(0)]);
        match denied.first() {
            Some((_, detail)) => assert_eq!(detail.as_slice(), first.as_bytes(), "{want}"),
            None => assert_eq!(refusals, 0, "{want}: audited"),
        }
        let counted: u32 = denied.iter().map(|&(count, _)| count).sum();
        assert_eq!(counted, refusals, "{want}: {denied:?}");
    }
    drop(host);
    assert_eq!(crate::cell::open_sockets(), 0);
}

/// Hostile (spec `networking`, "Ports of other domains"; design D6 of
/// `add-cell-hierarchy`): while a cell of another domain takes card ports
/// (connections to the outside) and loopback ports, an observer's own
/// ephemeral ports follow one another as when it runs alone. Not run
/// without a card and an echo server.
fn ports_of_other_domains_do_not_show() {
    let Some(echo) = crate::wire::options().echo else {
        arch::console::BootConsole.write_bytes(b"(no network card and echo server: not run) ");
        return;
    };
    let mut host = Host::new();
    assert!(
        host.wire_address(10_000_000_000).is_some(),
        "the card gets an address"
    );
    let observer = spawn(&mut host, "observer", &[b"ports"]);
    while host.output(observer) != Some(b"ready\n") {
        host.step();
    }
    let at = endpoint(echo);
    let neighbour = spawn(&mut host, "reach", &[b"take", b"100", at.as_bytes()]);
    assert!(host.set_egress(neighbour, crate::wire::echo_egress()));
    assert_ne!(host.domain(neighbour), host.domain(observer));
    while host.output(neighbour) != Some(b"holding\n") {
        assert!(
            host.finished(neighbour).is_none(),
            "{:?}",
            ended(&host, neighbour)
        );
        host.step();
    }
    assert!(host.wake(observer));
    host.run(|h| h.finished(observer).is_some());
    assert_eq!(
        ended(&host, observer),
        (Ending::Exit(0), "ready\nports +1\n".into())
    );
    assert!(host.wake(neighbour));
    host.run(|_| false);
    assert_eq!(
        ended(&host, neighbour),
        (Ending::Exit(0), "holding\n".into())
    );
}

/// Pipes, event counters, `poll`, `epoll`, `select` and sockets through a
/// real libc print what they print on Linux; every frame comes back.
fn sockets_and_pipes_behave_as_on_linux() {
    assert_no_leak("libc_net");
    for name in ["loopback", "piper", "netstate"] {
        run_expected(name);
    }
    assert_eq!(crate::cell::open_sockets(), 0, "every socket closed");
}

/// A server thread and a client thread of one cell exchange messages over
/// TCP on 127.0.0.1 (spec `networking`, "Echo inside a cell"); a client in
/// another cell is refused (the first half of "Echo between two cells":
/// the channel comes with phase 2 of `add-cell-hierarchy`).
fn cells_talk_over_loopback() {
    let mut host = Host::new();
    let pair = spawn(&mut host, "loopback", &[b"pair", b"7300", b"100"]);
    host.run(|_| false);
    assert_eq!(ended(&host, pair), (Ending::Exit(0), "paired\n".into()));
    let server = spawn(&mut host, "loopback", &[b"serve", b"7300", b"1"]);
    step_until_waiting(&mut host);
    let other = spawn(&mut host, "loopback", &[b"sibling", b"7300"]);
    host.run(|h| h.finished(other).is_some());
    assert_eq!(
        ended(&host, other),
        (Ending::Exit(0), "refused, then bound\n".into())
    );
    assert!(host.kill(server));
    assert_eq!(crate::cell::open_sockets(), 0);
}

/// Each cell has a loopback network of its own (spec `networking`,
/// "Separate networks"): a sibling's connection to a cell's listener on
/// 127.0.0.1:7300 is refused and it binds that port itself, while the two
/// threads of a third cell talk on that port too.
fn separate_loopback_networks() {
    let mut host = Host::new();
    let server = spawn(&mut host, "loopback", &[b"serve", b"7300", b"1"]);
    step_until_waiting(&mut host);
    let sibling = spawn(&mut host, "loopback", &[b"sibling", b"7300"]);
    let pair = spawn(&mut host, "loopback", &[b"pair", b"7300", b"10"]);
    host.run(|h| h.finished(sibling).is_some() && h.finished(pair).is_some());
    assert_eq!(
        ended(&host, sibling),
        (Ending::Exit(0), "refused, then bound\n".into())
    );
    assert_eq!(ended(&host, pair), (Ending::Exit(0), "paired\n".into()));
    assert!(host.finished(server).is_none(), "nobody reached the server");
    assert_eq!(host.waiting(), 1);
    assert!(host.kill(server));
    assert_eq!(crate::cell::open_sockets(), 0);
}

/// A server thread waiting in `accept` does not run while another cell
/// works; it runs once its cell's client thread, woken by the host,
/// connects.
fn a_waiting_cell_uses_no_cpu() {
    let mut host = Host::new();
    let pair = spawn(&mut host, "loopback", &[b"pair", b"7301", b"1", b"wait"]);
    step_until_waiting(&mut host); // in `accept`, its client asleep
    let chatter = spawn(&mut host, "chatter", &[]);
    host.run(|h| h.finished(chatter).is_some());
    assert!(host.finished(pair).is_none(), "the server still waits");
    assert_eq!(host.waiting(), 1, "and it is not runnable");
    assert!(host.wake(pair));
    host.run(|_| false);
    assert_eq!(ended(&host, pair), (Ending::Exit(0), "paired\n".into()));
}

/// A socket change wakes only the cells waiting on that socket (design D1
/// of `add-socket-wait-queues`): a server waiting for a client that never
/// comes stays asleep while the two threads of another cell talk, and no
/// wake-up finds nothing to do.
fn only_the_cells_waiting_on_a_socket_wake() {
    use crate::prof::{self, Count};
    let mut host = Host::new();
    let idle = spawn(&mut host, "loopback", &[b"serve", b"7310", b"1"]);
    step_until_waiting(&mut host); // it reaches `accept` and waits
    prof::start();
    let pair = spawn(&mut host, "loopback", &[b"pair", b"7311", b"50"]);
    host.run(|h| h.finished(pair).is_some());
    let report = prof::stop();
    assert_eq!(ended(&host, pair), (Ending::Exit(0), "paired\n".into()));
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
    host.run_to_pause(id, b"ready", Pause::Waits)
        .unwrap_or_else(|why| panic!("{why}"));
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

/// Runs `loopback` with `argv`, suspending it after every scheduling step
/// and resuming it at once, after `pause` when given (a cell run
/// meanwhile); returns its whole output and the number of resumes.
fn suspended_every_step(
    host: &mut Host,
    argv: &[&[u8]],
    egress: rhizo_net::Egress,
    pause: Option<&str>,
) -> (Vec<u8>, usize) {
    let mut id = spawn(host, "loopback", argv);
    assert!(host.set_egress(id, egress));
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
        if let Some(cell) = pause {
            let other = spawn(host, cell, &[]);
            host.run(|h| h.finished(other).is_some());
        }
        id = host
            .resume(snapshot, Resume::Lazy, true, true)
            .expect("resume");
        resumes += 1;
    }
    (output, resumes)
}

/// A loopback TCP connection between the two threads of a cell keeps
/// working while the cell, both ends with it, is suspended and resumed
/// after every scheduling step (its client yields after each round trip) (cells no longer share a loopback network,
/// design D5 of `add-cell-hierarchy`).
fn loopback_connection_across_suspend_of_one_end() {
    let mut host = Host::new();
    let (output, resumes) = suspended_every_step(
        &mut host,
        &[b"pair", b"7304", b"20", b"yield"],
        Default::default(),
        None,
    );
    assert!(resumes >= 20, "suspended {resumes} times");
    assert_eq!(alloc::string::String::from_utf8_lossy(&output), "paired\n");
    drop(host);
    assert_eq!(crate::cell::open_sockets(), 0);
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
        &[b"ping", port.as_bytes(), b"10", ip.as_bytes()],
        crate::wire::echo_egress(),
        Some("sleeper"),
    );
    assert!(resumes >= 10, "suspended {resumes} times");
    assert_eq!(alloc::string::String::from_utf8_lossy(&output), "pinged\n");
    drop(host);
    assert_eq!(crate::cell::open_sockets(), 0);
}

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

/// A server thread suspended while it waits in `accept` waits there again
/// after its cell's resume, and serves the cell's client thread, woken by
/// the host, when it comes.
fn a_blocked_cell_survives_suspend() {
    let mut host = Host::new();
    let pair = spawn(&mut host, "loopback", &[b"pair", b"7302", b"1", b"wait"]);
    step_until_waiting(&mut host);
    let snapshot = host.suspend(pair).expect("live");
    let pair = host
        .resume(snapshot, Resume::Eager, true, true)
        .expect("resume");
    step_until_waiting(&mut host);
    assert!(host.wake(pair));
    host.run(|_| false);
    assert_eq!(ended(&host, pair), (Ending::Exit(0), "paired\n".into()));
}

/// A cell whose server thread waits in `accept` is frozen as a template;
/// two cells spawned from it each listen again in their own network, and
/// the client thread of each (its sleep expired in the copy) is served by
/// its own cell's server only (spec `networking`, "Forked servers"): the
/// template's listener got no connection.
fn forked_servers_in_their_network() {
    let mut host = Host::new();
    let template = spawn(&mut host, "loopback", &[b"pair", b"7303", b"1", b"wait"]);
    step_until_waiting(&mut host);
    assert!(host.freeze(template));
    assert_eq!(crate::cell::open_sockets(), 1, "the template's listener");
    let copies: Vec<u32> = (0..2)
        .map(|_| {
            host.spawn_from_template(template, true, true)
                .expect("fork")
        })
        .collect();
    host.run(|_| false);
    for id in copies {
        assert_eq!(ended(&host, id), (Ending::Exit(0), "paired\n".into()));
    }
    assert_eq!(
        crate::cell::open_sockets(),
        1,
        "the template's listener, with no connection waiting on it"
    );
    drop(host);
    assert_eq!(
        crate::cell::open_sockets(),
        0,
        "the template's listener closed"
    );
}

/// Forked at `ready`, `netstate` and its child both read the pipe's bytes;
/// the child's connection is reset, the parent's is not, and both accept
/// on their listener, the child's bound again in its own network.
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
/// spawned from it finds the connection reset and accepts on its own
/// listener, bound again at the template's address in its network.
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
    // Egress `none` by default: it reaches the echo server by a rule.
    assert!(host.set_egress(id, crate::wire::echo_egress()));
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
    assert!(host.set_egress(id, crate::wire::echo_egress()));
    host.run(|_| false);
    crate::cell::set_wire_timeout(20_000_000_000);
    assert_eq!(ended(&host, id), (Ending::Exit(0), "quiet ok\n".into()));
}
