//! `bench-cell` with `profile`: where the time of a cell goes.

use super::*;

/// `bench-cell` with `profile`: the cost of a copy-on-write fault and the
/// breakdowns of the workloads (spec `profiling`), measured apart from the
/// other benchmarks, which run without the accounting hooks.
pub(super) fn profiled(console: &mut impl Console) {
    use core::fmt::Write;
    if crate::smp::count() > 1 {
        parallel_breakdown(console);
        return;
    }
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-ops cow_fault_ns={}",
        bench_ns(cow_fault_ticks()).unwrap_or(0)
    );
    breakdowns(console);
}

/// A frozen `ballast` cell holding `mib` MiB written, on `host`.
fn ballast_template(host: &mut Host, mib: &[u8]) -> u32 {
    let big = host
        .spawn(
            test_image("ballast"),
            &[b"ballast", b"write", mib],
            true,
            false,
        )
        .expect("spawn ballast");
    while host.output(big) != Some(b"filled\n") {
        host.step();
    }
    assert!(host.freeze(big));
    // Shipped by the host: its copies, each a root, share it (design D21
    // of `add-cell-hierarchy`).
    assert_eq!(host.publish(big, false), Ok(()));
    big
}

/// Counter ticks the kernel spends resolving one copy-on-write fault of a
/// 4 KiB page (frame, copy, entry and TLB flush; the trap itself is the
/// system call's round trip): that part accounted alone while a fork of a
/// cell holding 1 MiB (small pages) writes every page.
fn cow_fault_ticks() -> u64 {
    let mut host = Host::new();
    let big = ballast_template(&mut host, b"1");
    let mut samples: Vec<u64> = (0..ROUNDS)
        .map(|_| {
            let child = host.spawn_from_template(big, true, false).expect("fork");
            cost::start(1 << Part::Cow as u16);
            host.run(|h| h.finished(child).is_some());
            cost::stop(|a, _| a.inclusive(Part::Cow) / a.count(Part::Cow).max(1))
        })
        .collect();
    samples.sort_unstable();
    samples[ROUNDS / 2]
}

/// Rounds of each workload in a `sampling` build (one otherwise): more
/// samples.
const SAMPLED_ROUNDS: usize = if cfg!(feature = "sampling") { 5 } else { 1 };

/// Room for samples on the CPU that takes them (spec `profiling`).
#[cfg(feature = "sampling")]
const SAMPLE_ROOM: usize = 1 << 16;

/// Runs `f` with every part accounted and prints its breakdown (spec
/// `profiling`): the parts, the scopes entered, and the system calls that
/// took most time. `per_trap` is the cost of one entry into cell code and
/// back, in counter ticks. A `sampling` build also samples where the cells
/// (running `image`) are and prints the samples, folded.
fn breakdown(
    console: &mut impl Console,
    workload: &str,
    #[cfg_attr(not(feature = "sampling"), allow(unused_variables))] image: &str,
    per_trap: u64,
    f: impl FnOnce(),
) {
    use core::fmt::Write;
    cost::start(cost::ALL);
    #[cfg(feature = "sampling")]
    let (sampler, period) = cost::start_sampling(SAMPLE_ROOM);
    f();
    #[cfg(feature = "sampling")]
    cost::stop_sampling(|samples| {
        let mut w = rhizo_hal::ConsoleWriter(&mut *console);
        let _ = writeln!(
            w,
            "rhizo-samples {workload} image={image} sampler={sampler} period={period} lost={}",
            samples.lost()
        );
        let _ = rhizo_prof::samples::write(&mut w, workload, samples.sorted());
    });
    cost::stop(|account, calls| {
        let ns = |t: u64| timer::cycles_to_ns(t).unwrap_or(0);
        let mut w = rhizo_hal::ConsoleWriter(console);
        let _ = rhizo_prof::Breakdown {
            workload,
            account,
            per_trap,
        }
        .write(&mut w, ns);
        let _ = write!(w, "rhizo-breakdown-calls {workload}");
        let (top, n) = calls.top::<8>();
        for &(nr, count, ticks) in &top[..n] {
            let _ = match user::ABI.call_name(nr) {
                Some(name) => write!(w, " {name:?}={count}/{}", ns(ticks)),
                None => write!(w, " {nr}={count}/{}", ns(ticks)),
            };
        }
        let _ = writeln!(w);
    });
}

/// The breakdowns of the workloads (spec `profiling`): the cell benchmark
/// loop (`hello` spawned and run 101 times), a fork of a 50 MiB cell
/// writing every page, and CPython answering one request from cold.
fn breakdowns(console: &mut impl Console) {
    // One trap's cost: cell time per entry of a cell doing nothing but
    // system calls.
    let per_trap = {
        let mut host = Host::new();
        let argv: [&[u8]; 3] = [b"yielder", b"getpid", b"20000"];
        let id = host
            .spawn(test_image("yielder"), &argv, false, false)
            .expect("spawn yielder");
        cost::start(cost::ALL);
        host.run(|h| h.finished(id).is_some());
        cost::stop(|a, _| a.time(Part::Cell) / a.count(Part::Cell).max(1))
    };
    let mut host = Host::new();
    breakdown(console, "cell_loop", "hello", per_trap, || {
        for _ in 0..101 {
            let id = host
                .spawn(test_image("hello"), &[b"hello"], true, false)
                .expect("spawn");
            host.run(|h| h.finished(id).is_some());
        }
    });
    let big = ballast_template(&mut host, b"50");
    breakdown(console, "fork_touch", "ballast", per_trap, || {
        for _ in 0..SAMPLED_ROUNDS {
            let id = host.spawn_from_template(big, true, false).expect("fork");
            host.run(|h| h.finished(id).is_some());
        }
    });
    if let Some(files) = cpython_files() {
        const ARGV: [&[u8]; 2] = [b"/usr/bin/python3", b"/app/agent_demo.py"];
        breakdown(console, "cpython_cold", "cpython", per_trap, || {
            for _ in 0..SAMPLED_ROUNDS {
                let id = host
                    .spawn_with_files(test_image("cpython"), &ARGV, files.clone(), true, false)
                    .expect("spawn");
                host.run(|h| h.finished(id).is_some());
            }
        });
    }
}

/// With several CPUs: the breakdown of sixteen CPU-bound cells on every CPU
/// (time waiting on locks included), and how often each kernel lock made a
/// CPU wait meanwhile.
fn parallel_breakdown(console: &mut impl Console) {
    use core::fmt::Write;
    let locks = |host: &Host| {
        [
            ("frames", memory::contention()),
            ("heap", crate::heap::contention()),
            ("refs", SHARED.contention()),
            ("sharing", SHARING.contention()),
            ("net", NET.contention()),
            ("sched", host.shared.contention()),
        ]
    };
    let mut host = Host::new();
    let ids: Vec<u32> = (0..4 * crate::smp::count())
        .map(|_| {
            host.spawn(test_image("spin"), &[b"spin", b"5"], false, false)
                .expect("spawn")
        })
        .collect();
    let before = locks(&host);
    breakdown(console, "parallel", "spin", 0, || host.run(|_| false));
    let after = locks(&host);
    for id in ids {
        assert_eq!(host.finished(id).map(|f| f.ending), Some(Ending::Exit(0)));
    }
    let mut w = rhizo_hal::ConsoleWriter(console);
    let _ = write!(w, "rhizo-locks");
    for ((name, a), (_, b)) in after.iter().zip(&before) {
        let _ = write!(w, " {name}={}/{}", a.waits - b.waits, a.spins - b.spins);
    }
    let _ = writeln!(w);
}
