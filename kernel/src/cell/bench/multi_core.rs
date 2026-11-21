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
