//! `bench-cell`: single-CPU figures of cells: spawn, switches, system
//! calls, files, sockets, suspend and resume, fork and templates.

use super::*;

/// Medians over several rounds: forking a 50 MiB cell (frozen as a template,
/// so the child's writes copy everything), the fork plus the child writing
/// every page until it exits, the page-table frames a fork allocates, and
/// spawning `hello` from a template.
pub(super) fn fork_and_templates() -> [u64; 4] {
    const ROUNDS: usize = 7;
    let ns = |t0: u64, t1: u64| bench_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    let big = host
        .spawn(
            test_image("ballast"),
            &[b"ballast", b"write", b"50"],
            true,
            false,
        )
        .expect("spawn ballast");
    while host.output(big) != Some(b"filled\n") {
        host.step();
    }
    assert!(host.freeze(big));
    let (mut fork, mut touch) = (Vec::new(), Vec::new());
    let mut frames = 0;
    for _ in 0..ROUNDS {
        let before = memory::free_bytes();
        let t0 = bench_clock();
        let child = host.spawn_from_template(big, true, false).expect("fork");
        let t1 = bench_clock();
        frames = (before - memory::free_bytes()) / FRAME_SIZE;
        host.run(|h| h.finished(child).is_some());
        let t2 = bench_clock();
        assert_eq!(
            host.finished(child).map(|f| f.ending),
            Some(Ending::Exit(0))
        );
        fork.push(ns(t0, t1));
        touch.push(ns(t0, t2));
    }
    drop(host);
    let mut host = Host::new();
    let hello = host
        .spawn(test_image("hello"), &[b"hello"], true, false)
        .expect("spawn");
    assert!(host.freeze(hello));
    let mut template = Vec::new();
    for _ in 0..101 {
        let t0 = bench_clock();
        let id = host
            .spawn_from_template(hello, true, false)
            .expect("spawn from template");
        let t1 = bench_clock();
        host.run(|h| h.finished(id).is_some());
        template.push(ns(t0, t1));
    }
    let median = |v: &mut Vec<u64>| {
        v.sort_unstable();
        v[v.len() / 2]
    };
    [
        median(&mut fork),
        median(&mut touch),
        frames,
        robust(&mut template),
    ]
}

/// What the `threads` cell measures in its `bench` mode (design D10 of
/// `add-threads`): creating and joining a thread, and one futex hand-off
/// between two threads.
pub(super) fn threads() -> (u64, u64) {
    let mut host = Host::new();
    let id = host
        .spawn(test_image("threads"), &[b"threads", b"bench"], true, false)
        .expect("spawn threads");
    host.run(|h| h.finished(id).is_some());
    let done = host.finished(id).expect("ended");
    assert_eq!(
        done.ending,
        Ending::Exit(0),
        "the threads cell exits with 0"
    );
    let text = core::str::from_utf8(&done.output).unwrap_or("");
    let field = |key: &str| -> u64 {
        text.split_whitespace()
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    (field("create_ns"), field("futex_wake_ns"))
}

/// How far a 10 ms wait on a secondary CPU's timer is from 10 ms by the
/// counter (change `fix-secondary-cpu-timer`): the median of five waits on
/// each secondary CPU, the worst CPU's distance.
pub(super) fn timer_error() -> u64 {
    const WAIT_NS: u64 = 10_000_000;
    (1..crate::smp::count())
        .map(|cpu| {
            let mut took: Vec<u64> = (0..5)
                .map(|_| {
                    crate::smp::post(cpu, timed_wait, WAIT_NS);
                    crate::smp::join(cpu)
                })
                .collect();
            took.sort_unstable();
            took[took.len() / 2].abs_diff(WAIT_NS)
        })
        .max()
        .unwrap_or(0)
}

/// Robust figures (see [`robust`]): `filer` creating, writing and reading
/// back 1 MiB of file data, spawn to exit; and forking a cell (frozen as a
/// template) that holds a 16 MiB file.
pub(super) fn files() -> (u64, u64) {
    const RUNS: usize = 5 * ROUNDS;
    let image = test_image("filer");
    let ns = |t0: u64, t1: u64| bench_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    let mut write_read = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        host.finished.clear();
        let t0 = bench_clock();
        let id = host.spawn(image, &[b"filer"], true, false).expect("spawn");
        host.run(|h| h.finished(id).is_some());
        write_read.push(ns(t0, bench_clock()));
        let done = host.finished(id).expect("ended");
        assert_eq!(done.ending, Ending::Exit(0), "filer exits with 0");
        assert_eq!(done.output, b"files ok\n");
    }
    let holder = host
        .spawn(image, &[b"filer", b"16"], true, false)
        .expect("spawn");
    while host.output(holder) != Some(b"files ok\n") {
        host.step();
    }
    assert!(host.freeze(holder));
    let mut fork = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let t0 = bench_clock();
        let child = host.spawn_from_template(holder, true, false).expect("fork");
        fork.push(ns(t0, bench_clock()));
        assert!(host.kill(child));
    }
    (robust(&mut write_read), robust(&mut fork))
}

/// Robust figures (see [`robust`]) of the time per round trip: `piper`
/// writing one byte into a pipe and reading it back, and two `loopback`
/// cells exchanging a 64-byte message over TCP on 127.0.0.1 (design D7 of
/// `add-sockets-and-poll`); and of the time to resume a `loopback twice`
/// client suspended with its connection established until it has made one
/// more round trip and exited (design D5 of `keep-tcp-across-suspend`).
pub(super) fn net() -> (u64, u64, u64) {
    const RUNS: usize = 3 * ROUNDS;
    const PIPE_TRIPS: u64 = 2000;
    const NET_TRIPS: u64 = 200;
    let ns = |t0: u64, t1: u64| bench_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    let mut pipe = Vec::with_capacity(RUNS);
    let trips = b"2000";
    for _ in 0..RUNS {
        let t0 = bench_clock();
        let id = host
            .spawn(test_image("piper"), &[b"piper", trips], true, false)
            .expect("spawn piper");
        host.run(|h| h.finished(id).is_some());
        pipe.push(ns(t0, bench_clock()) / PIPE_TRIPS);
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    let image = test_image("loopback");
    let mut loopback = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        host.finished.clear();
        let server = host
            .spawn(
                image,
                &[b"loopback", b"serve", b"7400", b"200"],
                true,
                false,
            )
            .expect("spawn server");
        host.step(); // listening, waiting in `accept`
        let t0 = bench_clock();
        let client = host
            .spawn(image, &[b"loopback", b"ping", b"7400", b"200"], true, false)
            .expect("spawn client");
        host.run(|h| h.finished(server).is_some() && h.finished(client).is_some());
        loopback.push(ns(t0, bench_clock()) / NET_TRIPS);
        for id in [server, client] {
            assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
        }
    }
    let mut resume = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        host.finished.clear();
        let server = host
            .spawn(image, &[b"loopback", b"serve", b"7401", b"2"], true, false)
            .expect("spawn server");
        host.step(); // listening, waiting in `accept`
        let client = host
            .spawn(image, &[b"loopback", b"twice", b"7401"], true, false)
            .expect("spawn client");
        while host.output(client) != Some(b"once\n") {
            host.step();
        }
        let snapshot = host.suspend(client).expect("live");
        let t0 = bench_clock();
        let client = host
            .resume(snapshot, Resume::Lazy, true, false)
            .expect("resume");
        host.run(|h| h.finished(client).is_some());
        resume.push(ns(t0, bench_clock()));
        host.run(|h| h.finished(server).is_some());
        for id in [server, client] {
            assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
        }
    }
    (
        robust(&mut pipe),
        robust(&mut loopback),
        robust(&mut resume),
    )
}

/// A robust figure (see [`robust`]) of the round trip of a 64-byte message
/// between a `loopback` cell and the host's echo server, through the
/// network card (`echo=` on the command line).
pub(super) fn wire_rtt() -> Result<u64, alloc::string::String> {
    const RUNS: usize = 3 * ROUNDS;
    const TRIPS: u64 = 100;
    let (ip, port) = crate::wire::options()
        .echo
        .ok_or("no network card and echo server on this VMM")?;
    let mut host = Host::new();
    host.wire_address(10_000_000_000)
        .ok_or("the network card got no address")?;
    let ip = alloc::format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    let port = alloc::format!("{port}");
    let argv: [&[u8]; 5] = [b"loopback", b"ping", port.as_bytes(), b"100", ip.as_bytes()];
    let image = test_image("loopback");
    let mut samples = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        host.finished.clear();
        let t0 = bench_clock();
        let id = host.spawn(image, &argv, true, false).expect("spawn");
        // A connection that never answers must not stop the benchmarks.
        let end = now_ns().saturating_add(10_000_000_000);
        host.run_until(end, |h| h.finished(id).is_some());
        if host.finished(id).is_none() {
            let said = alloc::string::String::from_utf8_lossy(host.output(id).unwrap_or_default())
                .into_owned();
            host.kill(id);
            return Err(alloc::format!(
                "no answer from the echo server in 10 s: {}",
                said.trim().replace('\n', " | ")
            ));
        }
        match host.finished(id) {
            Some(f) if f.ending == Ending::Exit(0) => {}
            f => {
                let said =
                    f.map(|f| alloc::string::String::from_utf8_lossy(&f.output).into_owned());
                return Err(alloc::format!(
                    "the echo server did not answer: {}",
                    said.unwrap_or_default().trim().replace('\n', " | ")
                ));
            }
        }
        samples.push(bench_ns(bench_clock() - t0).unwrap_or(0) / TRIPS);
    }
    Ok(robust(&mut samples))
}

/// Medians over 7 rounds, for the `ballast` cell (8 MiB written): suspend,
/// lazy resume until runnable, eager resume, lazy resume plus touching every
/// page to the cell's exit; and the snapshot size; then the snapshot
/// exported to bytes with its checksum, and those bytes checked and decoded
/// (what saving and loading a snapshot file cost in the kernel, change
/// `fast-mem-primitives`).
pub(super) fn suspend_resume() -> [u64; 7] {
    const ROUNDS: usize = 7;
    let image = test_image("ballast");
    let ns = |t0: u64, t1: u64| bench_ns(t1 - t0).unwrap_or(0);
    let mut samples: [Vec<u64>; 6] = Default::default();
    let mut bytes = 0;
    let mut host = Host::new();
    for _ in 0..ROUNDS {
        for (eager, slot) in [(false, 1), (true, 2)] {
            let id = host
                .spawn(image, &[b"ballast"], true, false)
                .expect("spawn ballast");
            while host.output(id) != Some(b"filled\n") {
                host.step();
            }
            let t0 = bench_clock();
            let snapshot = host.suspend(id).expect("live");
            let t1 = bench_clock();
            bytes = snapshot.heap_bytes();
            if eager {
                let crc = crate::arch::mem::crc32c;
                let t0 = bench_clock();
                let exported = snapshot.export(&DirectMap, crc);
                let t1 = bench_clock();
                let decoded = Snapshot::from_bytes(exported, user::MACHINE, crc).expect("valid");
                let t2 = bench_clock();
                drop(decoded);
                samples[4].push(ns(t0, t1));
                samples[5].push(ns(t1, t2));
            }
            let mode = if eager { Resume::Eager } else { Resume::Lazy };
            let t2 = bench_clock();
            let id = host.resume(snapshot, mode, true, false).expect("resume");
            let t3 = bench_clock();
            host.run(|h| h.finished(id).is_some());
            let t4 = bench_clock();
            assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
            samples[0].push(ns(t0, t1));
            samples[slot].push(ns(t2, t3));
            if !eager {
                samples[3].push(ns(t2, t4));
            }
        }
    }
    let mut out = [0u64; 7];
    for (o, s) in out.iter_mut().zip(&mut samples) {
        s.sort_unstable();
        *o = s[s.len() / 2];
    }
    out.copy_within(4..6, 5);
    out[4] = bytes as u64;
    out
}

/// Spawns and runs the hello cell 101 times: robust spawn and spawn-to-end
/// times (see [`robust`]), and its page-table frames.
pub(super) fn hello_runs() -> (u64, u64, usize) {
    const RUNS: usize = 101;
    let image = test_image("hello");
    let mut spawn_ns = Vec::with_capacity(RUNS);
    let mut run_ns = Vec::with_capacity(RUNS);
    let mut tables = 0;
    let mut host = Host::new();
    for _ in 0..RUNS {
        host.finished.clear();
        let t0 = bench_clock();
        let id = host
            .spawn(image, &[b"hello"], true, false)
            .expect("the hello cell spawns");
        let t1 = bench_clock();
        tables = host.table_frames(id).unwrap_or(0);
        host.run(|_| false);
        let t2 = bench_clock();
        let done = host.finished(id).expect("the hello cell ended");
        assert_eq!(done.ending, Ending::Exit(0), "the hello cell exits with 0");
        assert_eq!(done.output, b"hello from a cell\n");
        spawn_ns.push(bench_ns(t1 - t0).unwrap_or(0));
        run_ns.push(bench_ns(t2 - t0).unwrap_or(0));
    }
    (robust(&mut spawn_ns), robust(&mut run_ns), tables)
}

/// Median over 5 rounds of the time per switch between two cells that
/// yield to each other 2000 times each (system call in, scheduler, system
/// call out to the other cell).
pub(super) fn switch_ns(mut host: Host) -> u64 {
    const YIELDS: u64 = 2000;
    let image = test_image("yielder");
    let mut rounds = [0u64; 5];
    for round in &mut rounds {
        let a = host
            .spawn(image, &[b"yielder"], false, false)
            .expect("spawn yielder");
        let b = host
            .spawn(image, &[b"yielder"], false, false)
            .expect("spawn yielder");
        let t0 = bench_clock();
        host.run(|h| h.finished(a).is_some() && h.finished(b).is_some());
        let t1 = bench_clock();
        *round = bench_ns(t1 - t0).unwrap_or(0) / (2 * YIELDS);
    }
    rounds.sort_unstable();
    rounds[2]
}

/// Median over 5 rounds of the time per switch between two `fp_regs`
/// cells that yield to each other 2000 times each with values in every
/// FP/SIMD register (change `lazy-fp-switch`): the run of both to their
/// exit, minus the same run with no yields, over the 4000 switches.
pub(super) fn switch_fp_ns() -> u64 {
    const YIELDS: u64 = 2000;
    let image = test_image("fp_regs");
    let mut host = Host::new();
    let mut pair = |rounds: &[u8]| {
        let a = host
            .spawn(image, &[b"fp_regs", b"1", rounds], true, false)
            .expect("spawn fp_regs");
        let b = host
            .spawn(image, &[b"fp_regs", b"2", rounds], true, false)
            .expect("spawn fp_regs");
        let t0 = bench_clock();
        host.run(|h| h.finished(a).is_some() && h.finished(b).is_some());
        let t1 = bench_clock();
        for id in [a, b] {
            let done = host.finished(id).expect("fp_regs ended");
            assert_eq!(done.output, b"fp regs ok\n", "fp_regs keeps its values");
        }
        t1 - t0
    };
    let mut rounds = [0u64; 5];
    for round in &mut rounds {
        let empty = pair(b"0");
        *round = pair(b"2000").saturating_sub(empty);
    }
    rounds.sort_unstable();
    bench_ns(rounds[2]).unwrap_or(0) / (2 * YIELDS)
}

/// Counter ticks of one `getpid` round trip (cell to kernel and back):
/// `yielder getpid 20000` run to its exit, minus `yielder getpid 0` (robust
/// over rounds, see [`robust`]).
pub(super) fn syscall_ticks() -> u64 {
    const CALLS: u64 = 20_000;
    let mut host = Host::new();
    let mut run = |n: &[u8]| {
        let argv: [&[u8]; 3] = [b"yielder", b"getpid", n];
        let id = host
            .spawn(test_image("yielder"), &argv, false, false)
            .expect("spawn yielder");
        let t0 = bench_clock();
        host.run(|h| h.finished(id).is_some());
        bench_clock() - t0
    };
    let mut samples: Vec<u64> = (0..3 * ROUNDS)
        .map(|_| {
            let empty = run(b"0");
            run(b"20000").saturating_sub(empty)
        })
        .collect();
    robust(&mut samples) / CALLS
}
