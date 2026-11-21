//! How cells share the CPUs: by weight between domains, within caps per
//! subtree (design D23 of `add-cell-hierarchy`).

use super::*;
use crate::cell::now_ns;
use rhizo_cell::quota::Quota;
use rhizo_sched::SLICE_NS;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(domains_share_by_weight, 45),
    ktest_case!(cpu_cap_holds, 25),
];

/// A spinning cell whose CPU time is counted (a CPU time quota it never
/// reaches), placed under `parent` with the default CPU weight and cap
/// `cpu_max`.
fn spinner(host: &mut Host, parent: Option<u32>, cpu_max: u32) -> u32 {
    let id = spawn(host, "spin", &[]);
    let quota = Quota {
        cpu_ns: u64::MAX - 1,
        ..Quota::NONE
    };
    assert!(host.set_quota(id, quota));
    assert!(host.set_cpu_share(id, parent, 100, cpu_max), "placed");
    id
}

/// The CPU time of `cells`, all live.
fn cpu_of(host: &mut Host, cells: &[u32]) -> u64 {
    cells.iter().map(|&c| host.cpu_ns(c).expect("live")).sum()
}

/// One CPU stepping through every cell (on any number of CPUs) for `ns`.
fn step_for(host: &mut Host, ns: u64) {
    let t0 = now_ns();
    while now_ns() - t0 < ns {
        host.step();
    }
}

/// Design D23: on one CPU, a domain of 16 spinning cells and a domain of
/// one, with equal weights, each get half the CPU within 5 % over 2 s
/// (round robin per cell would give the one cell a seventeenth).
fn domains_share_by_weight() {
    let mut host = Host::new();
    let root = spinner(&mut host, None, u32::MAX);
    let mut many = alloc::vec![root];
    for _ in 0..15 {
        many.push(spinner(&mut host, Some(root), u32::MAX));
    }
    let one = spinner(&mut host, None, u32::MAX);
    assert_eq!(host.domain(many[15]), host.domain(root), "one domain");
    step_for(&mut host, 2_000_000_000);
    let (a, b) = (cpu_of(&mut host, &many), cpu_of(&mut host, &[one]));
    let pct = 100 * b / (a + b);
    assert!(
        (45..=55).contains(&pct),
        "the one cell's domain got {pct} % ({b} ns of {})",
        a + b
    );
    for id in many.into_iter().rev().chain([one]) {
        assert!(host.kill(id));
    }
    assert!(host.tree().is_empty());
}

/// Spec `cell-hierarchy`, "Capped subtree": a subtree of two spinning
/// cells capped at 0.25 CPU, alone on idle CPUs, gets at most 25 % of a
/// CPU plus one slice per period for each CPU running it (one with
/// `ktest`, two with `ktest-smp`), on every CPU at once. The cap is
/// charged at the end of a slice, which lasts up to two timer ticks of
/// [`SLICE_NS`] (a cell is preempted at its second).
fn cpu_cap_holds() {
    const PERIOD: u64 = rhizo_sched::fair::PERIOD_NS;
    let mut host = Host::new();
    let root = spinner(&mut host, None, 250);
    let capped = [root, spinner(&mut host, Some(root), 250)];
    let t0 = now_ns();
    host.run_until(t0 + 1_000_000_000, |_| false);
    let elapsed = now_ns() - t0;
    let used = cpu_of(&mut host, &capped);
    let periods = elapsed.div_ceil(PERIOD) + 1;
    let running = (capped.len() as u64).min(crate::smp::count() as u64);
    let most = periods * (PERIOD / 4 + running * 2 * SLICE_NS);
    assert!(
        used <= most,
        "the capped subtree used {used} ns in {elapsed} ns (at most {most})"
    );
    assert!(
        used >= elapsed / 5,
        "it still ran: {used} ns in {elapsed} ns"
    );
    for id in capped.into_iter().rev() {
        assert!(host.kill(id));
    }
    assert!(host.tree().is_empty());
}
