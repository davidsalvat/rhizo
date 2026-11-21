//! `bench-cell`: real programs as workloads (CPython, NumPy, Node.js,
//! dynamically linked programs, the agent) and cell density.

use super::*;

/// numpy from its wheel (design D5 of `add-pip-packages`): the packages'
/// CPython started from its files, importing numpy and computing, run to
/// its exit after a first run that keeps the file pages; and forked from a
/// template frozen once numpy was imported (`ready`). Robust figures over
/// 10 runs each; `None` when the packages were not packed.
pub(super) fn numpy() -> Option<(u64, u64)> {
    const RUNS: usize = 10;
    const PATH: &[u8] = b"usr/bin/python3";
    const ARGV: [&[u8]; 3] = [PATH, b"-s", b"app/numpy_demo.py"];
    let files = packages_files()?;
    let ns = |t0: u64, t1: u64| timer::cycles_to_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    let mut cold = Vec::with_capacity(RUNS);
    for i in 0..=RUNS {
        host.finished.clear();
        let t0 = Counter.cycles();
        let id = host
            .spawn_file(files.clone(), PATH, &ARGV, &[], true, false)
            .expect("spawn");
        host.run(|h| h.finished(id).is_some());
        let t = ns(t0, Counter.cycles());
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
        if i > 0 {
            cold.push(t);
        }
    }
    let template = host
        .spawn_file(
            files,
            PATH,
            &[ARGV[0], ARGV[1], ARGV[2], b"wait"],
            &[],
            true,
            false,
        )
        .expect("spawn");
    host.run_to_pause(template, b"ready", Pause::Slice)
        .unwrap_or_else(|why| panic!("{why}"));
    assert!(host.freeze(template));
    // Shipped by the host: its copies, each a root, share it (design D21
    // of `add-cell-hierarchy`).
    assert_eq!(host.publish(template, false), Ok(()));
    let mut warm = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        host.finished.clear();
        let t0 = Counter.cycles();
        let id = host
            .spawn_from_template(template, true, false)
            .expect("fork");
        host.run(|h| h.finished(id).is_some());
        warm.push(ns(t0, Counter.cycles()));
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    Some((robust(&mut cold), robust(&mut warm)))
}

/// The dynamically linked `hello` (design D8 of `add-dynamic-linking`):
/// started from the cell's files and run to its exit, its interpreter
/// mapping `libc` and relocating, after a first run that keeps the file
/// pages; and forked from a template frozen once it printed `ready`, the
/// relocations done. Robust figures over 25 runs; `None` when the files
/// were not packed.
pub(super) fn dynamic() -> Option<(u64, u64)> {
    const RUNS: usize = 25;
    const PATH: &[u8] = b"/usr/bin/hello";
    let files = dynamic_files()?;
    let ns = |t0: u64, t1: u64| timer::cycles_to_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    let mut run = Vec::with_capacity(RUNS);
    for i in 0..=RUNS {
        host.finished.clear();
        let t0 = Counter.cycles();
        let id = host
            .spawn_file(files.clone(), PATH, &[b"hello"], &[], true, false)
            .expect("spawn");
        host.run(|h| h.finished(id).is_some());
        let t = ns(t0, Counter.cycles());
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
        if i > 0 {
            run.push(t);
        }
    }
    let template = host
        .spawn_file(files, PATH, &[b"hello", b"wait"], &[], true, false)
        .expect("spawn");
    host.run_to_pause(template, b"ready", Pause::Waits)
        .unwrap_or_else(|why| panic!("{why}"));
    assert!(host.freeze(template));
    // Shipped by the host: its copies, each a root, share it (design D21
    // of `add-cell-hierarchy`).
    assert_eq!(host.publish(template, false), Ok(()));
    let mut warm = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        host.finished.clear();
        let t0 = Counter.cycles();
        let id = host
            .spawn_from_template(template, true, false)
            .expect("fork");
        host.run(|h| h.finished(id).is_some());
        warm.push(ns(t0, Counter.cycles()));
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    Some((robust(&mut run), robust(&mut warm)))
}

/// A real program (`libc_warm`, static glibc) answering one request: started
/// cold from its executable (warm-up included), and forked from a template
/// frozen after its warm-up. Medians over 7 rounds.
pub(super) fn workload() -> (u64, u64) {
    const ROUNDS: usize = 7;
    let image = test_image("libc_warm");
    let ns = |t0: u64, t1: u64| bench_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    let (mut cold, mut warm) = (Vec::new(), Vec::new());
    for _ in 0..ROUNDS {
        let t0 = bench_clock();
        let id = host
            .spawn(image, &[b"libc_warm"], true, false)
            .expect("spawn");
        host.run(|h| h.finished(id).is_some());
        cold.push(ns(t0, bench_clock()));
    }
    let template = host
        .spawn(image, &[b"libc_warm"], true, false)
        .expect("spawn");
    host.run_to_pause(template, b"warm", Pause::Slice)
        .unwrap_or_else(|why| panic!("{why}"));
    assert!(host.freeze(template));
    // Shipped by the host: its copies, each a root, share it (design D21
    // of `add-cell-hierarchy`).
    assert_eq!(host.publish(template, false), Ok(()));
    for _ in 0..ROUNDS {
        let t0 = bench_clock();
        let id = host
            .spawn_from_template(template, true, false)
            .expect("fork");
        host.run(|h| h.finished(id).is_some());
        warm.push(ns(t0, bench_clock()));
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    cold.sort_unstable();
    warm.sort_unstable();
    (cold[ROUNDS / 2], warm[ROUNDS / 2])
}

/// MicroPython answering one request (`cells/python/rhizo_demo.py`): started
/// cold from its executable (interpreter start, imports, warm-up), and
/// forked from a template frozen after the warm-up. Medians over 7 rounds;
/// `None` when MicroPython was not built.
pub(super) fn python() -> Option<(u64, u64)> {
    const ROUNDS: usize = 7;
    const COMMAND: &[u8] =
        b"import sys; sys.path.insert(0, 'lib'); import rhizo_demo; rhizo_demo.main()";
    let image = test_image("micropython");
    if image.is_empty() {
        return None;
    }
    let files = || {
        let mut fs = Fs::new();
        fs.put_file(
            b"/lib/rhizo_demo.py",
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../cells/python/rhizo_demo.py"
            )),
        )
        .expect("module");
        fs
    };
    let argv: [&[u8]; 3] = [b"micropython", b"-c", COMMAND];
    let ns = |t0: u64, t1: u64| bench_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    let (mut cold, mut warm) = (Vec::new(), Vec::new());
    for _ in 0..ROUNDS {
        let t0 = bench_clock();
        let id = host
            .spawn_with_files(image, &argv, files(), true, false)
            .expect("spawn");
        host.run(|h| h.finished(id).is_some());
        cold.push(ns(t0, bench_clock()));
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    let template = host
        .spawn_with_files(image, &argv, files(), true, false)
        .expect("spawn");
    host.run_to_pause(template, b"warm", Pause::Waits)
        .unwrap_or_else(|why| panic!("{why}"));
    assert!(host.freeze(template));
    // Shipped by the host: its copies, each a root, share it (design D21
    // of `add-cell-hierarchy`).
    assert_eq!(host.publish(template, false), Ok(()));
    for _ in 0..ROUNDS {
        let t0 = bench_clock();
        let id = host
            .spawn_from_template(template, true, false)
            .expect("fork");
        host.run(|h| h.finished(id).is_some());
        warm.push(ns(t0, bench_clock()));
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    cold.sort_unstable();
    warm.sort_unstable();
    Some((cold[ROUNDS / 2], warm[ROUNDS / 2]))
}

/// Node.js answering one tool call (`cells/node/demo.js`; change
/// `add-node-workload`), with `--jitless` or with its JIT (`jit`, change
/// `node-jit`): spawned cold from its files (`ld.so`, Node's start-up,
/// warm-up, answer, exit), and forked from a template frozen after its
/// warm-up. Medians over 7 rounds, after a first spawn that fills the page
/// cache, and the changes of write-or-execute pages in one more cold run;
/// `None` when Node.js was not packed.
pub(super) fn node(jit: bool) -> Option<(u64, u64, u64)> {
    const ROUNDS: usize = 7;
    let argv: &[&[u8]] = match jit {
        false => &[b"/usr/bin/node", b"--jitless", b"/app/demo.js", b"wait"],
        true => &[b"/usr/bin/node", b"/app/demo.js", b"wait"],
    };
    let script = argv.len() - 1; // without `wait`
    let files = node_files()?;
    let ns = |t0: u64, t1: u64| bench_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    // Cold runs do not pause; the template does, after `warm`.
    let spawn = |host: &mut Host, argv: &[&[u8]]| {
        host.spawn_file(files.clone(), argv[0], argv, &[], true, false)
            .expect("spawn")
    };
    let (mut cold, mut warm) = (Vec::new(), Vec::new());
    for round in 0..=ROUNDS {
        host.finished.clear();
        let t0 = bench_clock();
        let id = spawn(&mut host, &argv[..script]);
        host.run(|h| h.finished(id).is_some());
        if round > 0 {
            cold.push(ns(t0, bench_clock()));
        }
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    // Counting slows system calls a little: a run of its own.
    prof::start();
    let id = spawn(&mut host, &argv[..script]);
    host.run(|h| h.finished(id).is_some());
    let flips = prof::stop().count(Count::WxFlips);
    host.finished.clear();
    // Frozen in its pause after `warm`, every thread waiting (the main one
    // in `epoll_pwait`, for the timer), not wherever the slice that printed
    // `warm` ended: the same state on every VMM (design D2 of
    // `fast-node-template`).
    let template = spawn(&mut host, argv);
    host.run_to_pause(template, b"warm", Pause::Waits)
        .unwrap_or_else(|why| panic!("{why}"));
    assert!(host.freeze(template));
    // Shipped by the host: its copies, each a root, share it (design D21
    // of `add-cell-hierarchy`).
    assert_eq!(host.publish(template, false), Ok(()));
    // The pause (50 ms) ends before the first fork, as for a template made
    // long before: a fork's `epoll_pwait` has timed out, but libuv compares
    // the timer's due time with the clock and would wait for the rest.
    let end = now_ns() + 50_000_000;
    while now_ns() < end {}
    for _ in 0..ROUNDS {
        host.finished.clear();
        let t0 = bench_clock();
        let id = host
            .spawn_from_template(template, true, false)
            .expect("fork");
        host.run(|h| h.finished(id).is_some());
        warm.push(ns(t0, bench_clock()));
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    cold.sort_unstable();
    warm.sort_unstable();
    Some((cold[ROUNDS / 2], warm[ROUNDS / 2], flips))
}

/// CPython answering one tool call (`cells/python/agent_demo.py`, design D3
/// of `add-cpython`): started cold from its executable (interpreter start,
/// imports, warm-up) with the standard library in its files, and forked from
/// a template frozen after the warm-up. Medians over 7 rounds; `None` when
/// CPython was not built.
pub(super) fn cpython() -> Option<(u64, u64)> {
    const ROUNDS: usize = 7;
    const ARGV: [&[u8]; 2] = [b"/usr/bin/python3", b"/app/agent_demo.py"];
    let image = test_image("cpython");
    let files = cpython_files()?;
    let ns = |t0: u64, t1: u64| bench_ns(t1 - t0).unwrap_or(0);
    let mut host = Host::new();
    let (mut cold, mut warm) = (Vec::new(), Vec::new());
    for _ in 0..ROUNDS {
        let t0 = bench_clock();
        let id = host
            .spawn_with_files(image, &ARGV, files.clone(), true, false)
            .expect("spawn");
        host.run(|h| h.finished(id).is_some());
        cold.push(ns(t0, bench_clock()));
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    let template = host
        .spawn_with_files(image, &ARGV, files, true, false)
        .expect("spawn");
    host.run_to_pause(template, b"warm", Pause::Waits)
        .unwrap_or_else(|why| panic!("{why}"));
    assert!(host.freeze(template));
    // Shipped by the host: its copies, each a root, share it (design D21
    // of `add-cell-hierarchy`).
    assert_eq!(host.publish(template, false), Ok(()));
    for _ in 0..ROUNDS {
        let t0 = bench_clock();
        let id = host
            .spawn_from_template(template, true, false)
            .expect("fork");
        host.run(|h| h.finished(id).is_some());
        warm.push(ns(t0, bench_clock()));
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    cold.sort_unstable();
    warm.sort_unstable();
    Some((cold[ROUNDS / 2], warm[ROUNDS / 2]))
}

/// The CPython agent's figures (design D4 of `add-agent-workload`), over 3
/// pairs of runs of 8 turns: medians of a turn as the agent times it and of
/// the part of it spent on the model's answer (the first turn, which
/// connects and runs cold, left out) from runs that are not suspended, as
/// the Linux process is not; of a suspend plus eager resume after every
/// turn of the other runs; and of a fork into the branches until they run,
/// from both.
pub(super) fn agent_figures() -> Result<[u64; 4], alloc::string::String> {
    const ROUNDS: usize = 3;
    const TURNS: usize = 8;
    let mut samples: [Vec<u64>; 4] = Default::default();
    for _ in 0..ROUNDS {
        let mut host = Host::new();
        let suspended = agent(
            &mut host,
            TURNS,
            Some(Resume::Eager),
            b"fake",
            AgentOutput::Timed,
        )?;
        samples[2].extend(suspended.suspend_resume_ns);
        samples[3].push(suspended.fork_ns);
        drop(host);
        let mut host = Host::new();
        let run = agent(&mut host, TURNS, None, b"fake", AgentOutput::Timed)?;
        let text = alloc::string::String::from_utf8_lossy(&run.transcript);
        for (key, slot) in [("agent turn_ns=", 0), ("agent model_ns=", 1)] {
            let times = text
                .lines()
                .find_map(|l| l.strip_prefix(key))
                .ok_or("the agent printed no turn times")?;
            samples[slot].extend(
                times
                    .split(',')
                    .skip(1)
                    .filter_map(|t| t.parse::<u64>().ok()),
            );
        }
        samples[3].push(run.fork_ns);
    }
    let mut out = [0; 4];
    for (o, s) in out.iter_mut().zip(&mut samples) {
        s.sort_unstable();
        *o = s.get(s.len() / 2).copied().unwrap_or(0);
    }
    Ok(out)
}

/// Memory per idle cell, from 1000 forks of a template of `idle` each run
/// into its sleep (the second of two identical rounds, so the heap has
/// grown its tables already), and the snapshot size of one idle cell
/// (design D3 of `add-memory-accounting`).
pub(super) fn density() -> (u64, u64) {
    const N: usize = 1000;
    let image = test_image("idle");
    let round = || {
        let mut host = Host::new();
        let template = host.spawn(image, &[b"idle"], true, false).expect("spawn");
        while host.output(template) != Some(b"idle\n") {
            host.step();
        }
        assert!(host.freeze(template));
        // Shipped by the host: its copies, each a root, share it (design D21
        // of `add-cell-hierarchy`).
        assert_eq!(host.publish(template, false), Ok(()));
        let before = memory::free_bytes();
        let ids: Vec<u32> = (0..N)
            .map(|_| {
                host.spawn_from_template(template, false, false)
                    .expect("fork")
            })
            .collect();
        for _ in 0..N {
            host.step(); // each child runs into its sleep
        }
        let per_cell = (before - memory::free_bytes()) / N as u64;
        for id in ids {
            host.kill(id);
        }
        per_cell
    };
    round();
    let idle = round();
    let mut host = Host::new();
    let id = host.spawn(image, &[b"idle"], true, false).expect("spawn");
    while host.output(id) != Some(b"idle\n") {
        host.step();
    }
    let suspended = host.suspend(id).map_or(0, |s| s.heap_bytes() as u64);
    (idle, suspended)
}
