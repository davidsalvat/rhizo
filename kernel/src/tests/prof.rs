//! Performance counters and cost accounting (spec `profiling`).

use super::*;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(performance_counters_count_where_present),
    ktest_case!(cost_accounting_adds_up_over_a_cell),
    ktest_case!(samples_show_where_a_cell_spins),
];

/// Where the CPU or hypervisor has performance counters, they count cycles
/// (and retired instructions where counted) in kernel code; without them
/// there is nothing to read (spec `profiling`).
fn performance_counters_count_where_present() {
    let Some(_) = arch::pmu::init() else {
        assert!(arch::pmu::read().is_none());
        return;
    };
    let before = arch::pmu::read().expect("counters on");
    let mut x = 0u64;
    for i in 0..10_000u64 {
        x = core::hint::black_box(x.wrapping_add(i));
    }
    let counted = arch::pmu::read().expect("counters on").since(before);
    assert!(counted.cycles > 0, "cycles did not advance");
    if let Some(instructions) = counted.instructions {
        assert!(instructions >= 10_000, "{instructions} instructions");
    }
}

/// With every part accounted, a cell making 100 `getpid` calls shows up as
/// cell time, system calls (100 of them `getpid`), spawn, teardown, frames
/// and copies, and the parts add up to no more than the time it took on
/// every CPU
/// (spec `profiling`).
fn cost_accounting_adds_up_over_a_cell() {
    use rhizo_prof::Part;
    let t0 = arch::timer::Counter.cycles();
    crate::cost::start(crate::cost::ALL);
    let (ending, _) = crate::cell::run(
        crate::cell::test_image("yielder"),
        &[b"yielder", b"getpid", b"100"],
    )
    .expect("spawn yielder");
    crate::cost::stop(|account, calls| {
        let elapsed = arch::timer::Counter.cycles() - t0;
        assert_eq!(ending, Ending::Exit(0));
        // Every CPU accounts its own time.
        let cpus = crate::smp::count() as u64;
        assert!(account.total() <= cpus * elapsed, "more time than elapsed");
        assert!(account.count(Part::Syscall) >= 101, "100 getpid and exit");
        assert!(account.count(Part::Cell) >= account.count(Part::Syscall));
        for part in [
            Part::Cell,
            Part::Syscall,
            Part::Spawn,
            Part::Teardown,
            Part::Frames,
            Part::Copy,
        ] {
            assert!(account.time(part) > 0, "no time in {}", part.name());
        }
        let (top, n) = calls.top::<8>();
        let getpid = top[..n]
            .iter()
            .find(|c| {
                alloc::format!("{:?}", arch::user::ABI.call_name(c.0).expect("known")) == "Getpid"
            })
            .expect("getpid recorded");
        assert_eq!(getpid.1, 100);
    });
}

/// Sampling (spec `profiling`): while a cell spins, every sample taken is
/// that cell's, at an address of its code, with the PMU's overflow
/// interrupt or, without one, the timer.
fn samples_show_where_a_cell_spins() {
    let mut host = Host::new();
    let id = spawn(&mut host, "spin", &[b"20"]);
    let (sampler, _) = crate::cost::start_sampling(4096);
    host.run(|h| h.finished(id).is_some());
    let samples = crate::cost::stop_sampling(|s| s.sorted().to_vec());
    assert_eq!(ended(&host, id).0, Ending::Exit(0));
    assert!(!samples.is_empty(), "no sample ({sampler})");
    for s in &samples {
        assert_eq!(s.cell, id, "{sampler}");
        assert!(s.pc < 0x0000_8000_0000_0000, "{:#x} ({sampler})", s.pc);
    }
}
