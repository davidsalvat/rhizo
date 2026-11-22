//! `bench-cell`: figures on several CPUs.

use super::*;

/// With several CPUs (design D8 of `add-smp-scheduling`): the time for
/// four CPU-bound cells per CPU (`spin 5`) to run to their end on this CPU
/// alone (steps) and on every CPU (a run), medians over 3 rounds; and the
/// round trip to an idle CPU (wake it, run nothing, see it done), robust
/// figure over 105.
pub(super) fn multi_core() -> (u64, u64, u64, u64, u64) {
    const ROUNDS: usize = 3;
    let ns = |t0: u64, t1: u64| bench_ns(t1 - t0).unwrap_or(0);
    let batch = |argv: &[&[u8]], parallel: bool| {
        let image = test_image(core::str::from_utf8(argv[0]).unwrap_or("spin"));
        let mut host = Host::new();
        let ids: Vec<u32> = (0..4 * crate::smp::count())
            .map(|_| host.spawn(image, argv, false, false).expect("spawn"))
            .collect();
        let t0 = bench_clock();
        if parallel {
            host.run(|_| false);
        } else {
            while host.step() {}
        }
        let t1 = bench_clock();
        for id in ids {
            assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
        }
        ns(t0, t1)
    };
    let median = |argv: &[&[u8]], parallel: bool| {
        let mut t: Vec<u64> = (0..ROUNDS).map(|_| batch(argv, parallel)).collect();
        t.sort_unstable();
        t[ROUNDS / 2]
    };
    let spin: [&[u8]; 2] = [b"spin", b"5"];
    let (serial, parallel) = (median(&spin, false), median(&spin, true));
    // System calls of different cells (design D3 of `split-sockets-lock`):
    // each `piper` writes and reads a byte through its pipe 20,000 times.
    let piper: [&[u8]; 2] = [b"piper", b"20000"];
    let (calls_serial, calls_parallel) = (median(&piper, false), median(&piper, true));
    fn nothing(x: u64) -> u64 {
        x
    }
    let mut wake: Vec<u64> = (0..105)
        .map(|_| {
            let t0 = bench_clock();
            crate::smp::post(1, nothing, 0);
            crate::smp::join(1);
            ns(t0, bench_clock())
        })
        .collect();
    (
        serial,
        parallel,
        robust(&mut wake),
        calls_serial,
        calls_parallel,
    )
}

/// How long another cell stalls while the host works on one (design D6 of
/// `host-ops-per-cell`): `progress` and a spinner run; in each round the
/// host forks the spinner, kills the child, suspends the spinner and
/// resumes it. The figure is the median over rounds of the longest gap
/// `progress` saw from just before the round's operations until it has
/// counted again after them: with the operations made from the run's stop
/// condition, and, for comparison, between runs (every CPU stopped, as
/// host operations were before that change).
pub(super) fn host_op_stall() -> (u64, u64) {
    const ROUNDS: usize = 21;
    fn operate(host: &mut Host, spinner: &mut u32) {
        let child = host.fork(*spinner, 1).expect("fork")[0];
        assert!(host.kill(child));
        let snapshot = host.suspend(*spinner).expect("suspend");
        *spinner = host
            .resume(snapshot, Resume::Lazy, false, false)
            .expect("resume");
    }
    let mut host = Host::new();
    // The boot CPU looks often even while the others run every cell.
    host.look_every(Some(1_000_000));
    let progress = host
        .spawn(
            test_image("progress"),
            &[b"progress".as_slice()],
            true,
            false,
        )
        .expect("spawn progress");
    host.run(|h| h.output(progress).is_some_and(|o| o.ends_with(b"\n")));
    let addr = host
        .output(progress)
        .and_then(|o| core::str::from_utf8(o).ok()?.trim().strip_prefix("at 0x"))
        .and_then(|hex| u64::from_str_radix(hex, 16).ok())
        .expect("the address of its words");
    let watched = host.watch(progress, addr, 2).expect("its own page");
    let (count, gap) = (watched.word(0), watched.word(1));
    let mut spinner = host
        .spawn(test_image("spin"), &[b"spin".as_slice()], false, false)
        .expect("spawn spin");
    let median = |gaps: &mut Vec<u64>| {
        gaps.sort_unstable();
        timer::cycles_to_ns(gaps[gaps.len() / 2]).unwrap_or(0)
    };
    // From the stop condition: a round operates, then its gap is read once
    // `progress` has counted again.
    let (mut running, mut calls, mut after) = (Vec::new(), 0, None);
    host.run(|h| {
        calls += 1;
        match after {
            _ if calls == 1 => {} // before the other CPUs start
            None => {
                gap.store(0, Ordering::Relaxed);
                operate(h, &mut spinner);
                after = Some(count.load(Ordering::Relaxed));
            }
            Some(c) if count.load(Ordering::Relaxed) > c => {
                running.push(gap.load(Ordering::Relaxed));
                after = None;
            }
            Some(_) => {}
        }
        running.len() == ROUNDS
    });
    // Between runs: the run ends, the host operates, and the next run goes
    // on until `progress` has counted again.
    let mut stopped = Vec::new();
    for _ in 0..ROUNDS {
        gap.store(0, Ordering::Relaxed);
        operate(&mut host, &mut spinner);
        let c = count.load(Ordering::Relaxed);
        host.run(|_| count.load(Ordering::Relaxed) > c);
        stopped.push(gap.load(Ordering::Relaxed));
    }
    assert!(host.kill(spinner) && host.kill(progress));
    (median(&mut running), median(&mut stopped))
}

/// With several CPUs (design D9 of `add-parallel-threads`): `parsum` with 1
/// and with 4 threads, alone on every CPU (the time it reports, medians
/// over 3 rounds), and the median `munmap` of `shootdown bench` while its
/// other threads run on the other CPUs.
pub(super) fn parallel_threads() -> (u64, u64, u64) {
    const ROUNDS: usize = 3;
    // Both print the figure last: `ns <n>`, `shootdown_ns=<n>`.
    let run = |argv: &[&[u8]]| -> u64 {
        let name = core::str::from_utf8(argv[0]).unwrap_or("");
        let mut host = Host::new();
        let id = host
            .spawn(test_image(name), argv, true, false)
            .expect("spawn");
        host.run(|h| h.finished(id).is_some());
        let done = host.finished(id).expect("ended");
        assert_eq!(done.ending, Ending::Exit(0), "{name}");
        let text = core::str::from_utf8(&done.output).unwrap_or("");
        let last = text.trim_end().rsplit([' ', '=']).next();
        last.and_then(|v| v.parse().ok()).unwrap_or(0)
    };
    let median = |threads: &[u8]| {
        let mut ns: Vec<u64> = (0..ROUNDS)
            .map(|_| run(&[b"parsum", threads, b"24", b"time"]))
            .collect();
        ns.sort_unstable();
        ns[ROUNDS / 2]
    };
    let (one, four) = (median(b"1"), median(b"4"));
    let shootdown = run(&[b"shootdown", b"bench", b"200"]);
    (one, four, shootdown)
}

/// With exclusive CPU sets (design D17 of `add-cell-hierarchy`): the
/// batch of [`multi_core`] (four `spin 5` cells per CPU) on every CPU, the
/// cells of each CPU from 2 on in a domain of their own with that CPU as
/// its exclusive set (a root and three copies of it, roots of its domain),
/// the others on CPUs 0 and 1; median over 3 rounds.
pub(super) fn parallel_sets() -> u64 {
    const ROUNDS: usize = 3;
    let image = test_image("spin");
    let argv: [&[u8]; 2] = [b"spin", b"5"];
    let cpus = crate::smp::count();
    let mut t: Vec<u64> = (0..ROUNDS)
        .map(|_| {
            let mut host = Host::new();
            let mut ids = Vec::new();
            for cpu in 2..cpus {
                let root = host.spawn(image, &argv, false, false).expect("spawn");
                let set = rhizo_cap::Spec {
                    cpus: 1 << cpu,
                    ..rhizo_cap::Spec::default()
                };
                host.place(root, rhizo_cap::HOST, set, 0).expect("its set");
                ids.extend(host.fork(root, 3).expect("copies"));
                ids.push(root);
            }
            while ids.len() < 4 * cpus {
                ids.push(host.spawn(image, &argv, false, false).expect("spawn"));
            }
            let t0 = bench_clock();
            host.run(|_| false);
            let t1 = bench_clock();
            for id in ids {
                assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
            }
            bench_ns(t1 - t0).unwrap_or(0)
        })
        .collect();
    t.sort_unstable();
    t[ROUNDS / 2]
}

/// Period of one bit of the CPU-contention channel: four slices (a
/// sender woken at the start of a period may wait a slice for the CPU).
const COVERT_PERIOD_NS: u64 = 4 * rhizo_sched::SLICE_NS;
/// Bits sent over it.
const COVERT_BITS: u64 = 100;

/// A CPU-contention channel between two domains (design D12 and D17 of
/// `add-cell-hierarchy`, `covert.cpu_bps`): `colluder bits` spins through
/// a period for a 1 and sleeps through it for a 0, `colluder count` counts
/// its loops in each period and decodes; each a root of its own domain.
/// On the boot CPU alone (both share it), then with an exclusive set of one
/// CPU each (CPUs 2 and 3). Returns the period, the bits, and the
/// receiver's errors without and with sets (0 with sets on fewer than
/// four CPUs, where two sets of one CPU do not fit).
pub(super) fn covert() -> (u64, u64, u64, u64) {
    let image = test_image("colluder");
    let round = |sets: bool| -> u64 {
        let mut host = Host::new();
        let start = alloc::format!("{}", now_ns() + 100_000_000);
        let period = alloc::format!("{COVERT_PERIOD_NS}");
        let bits = alloc::format!("{COVERT_BITS}");
        let args = |mode: &'static [u8]| -> [&[u8]; 5] {
            [
                b"colluder",
                mode,
                start.as_bytes(),
                period.as_bytes(),
                bits.as_bytes(),
            ]
        };
        let sender = host
            .spawn(image, &args(b"bits"), false, false)
            .expect("spawn");
        let receiver = host
            .spawn(image, &args(b"count"), true, false)
            .expect("spawn");
        let done = |h: &Host| h.finished(sender).is_some() && h.finished(receiver).is_some();
        if sets {
            for (cell, cpu) in [(sender, 2), (receiver, 3)] {
                let set = rhizo_cap::Spec {
                    cpus: 1 << cpu,
                    ..rhizo_cap::Spec::default()
                };
                host.place(cell, rhizo_cap::HOST, set, 0).expect("its set");
            }
            // A run may give back the CPUs while both sleep to their start.
            while !done(&host) {
                host.run(|h| done(h));
            }
        } else {
            while !done(&host) {
                host.step();
            }
        }
        let out = &host.finished(receiver).expect("ended").output;
        let text = core::str::from_utf8(out).unwrap_or("");
        text.trim_end()
            .rsplit('=')
            .next()
            .and_then(|e| e.parse().ok())
            .unwrap_or_else(|| panic!("covert receiver said {text:?}"))
    };
    let shared = round(false);
    let sets = if crate::smp::count() >= 4 {
        round(true)
    } else {
        0
    };
    (COVERT_PERIOD_NS, COVERT_BITS, shared, sets)
}
