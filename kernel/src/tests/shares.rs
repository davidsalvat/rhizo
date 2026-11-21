//! How cells share the CPUs: by weight between domains, within caps per
//! subtree (design D23 of `add-cell-hierarchy`).

use super::*;
use crate::cell::now_ns;
use rhizo_cap::{Actor, HOST, Limits, Spec};
use rhizo_cell::quota::Quota;
use rhizo_sched::SLICE_NS;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(domains_share_by_weight, 45),
    ktest_case!(root_weights_share_a_cpu, 25),
    ktest_case!(cpu_cap_holds, 25),
];

const MIB: u64 = 1 << 20;

/// A root's limits: 1 GiB, CPU time counted (a quota no test reaches),
/// and room for `cells` descendants.
fn parent(host: &mut Host, id: u32, cells: u32) {
    let quota = Quota {
        memory: 1 << 30,
        cpu_ns: u64::MAX - 1,
        ..Quota::DEFAULT
    };
    assert!(host.set_limits(id, quota, cells), "limits of {id}");
}

/// A child's spec: `mib` MiB, `cpu_ns` of CPU time (finite: counted), one
/// socket per cell, `cells` descendants, and CPU cap `cpu_max`
/// (thousandths of a CPU, `u32::MAX`: none).
fn child(mib: u64, cpu_ns: u64, cells: u32, cpu_max: u32) -> Spec {
    Spec {
        limits: Limits {
            memory: mib * MIB,
            cpu_ns,
            sockets: cells + 1,
            cells,
        },
        cpu_max,
        ..Spec::default()
    }
}

/// A spinning cell placed by the host under `under` with `spec`.
fn spinner_under(host: &mut Host, under: u32, spec: Spec) -> u32 {
    let id = spawn(host, "spin", &[]);
    host.adopt(id, under, spec, Actor::Host).expect("placed");
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

/// Spec `cell-hierarchy`, "Many cells do not buy CPU": on one CPU, a
/// domain of 16 spinning cells (a root and 15 children) and a domain of
/// one, with equal weights, each get half the CPU within 5 % over 2 s
/// (round robin per cell would give the one cell a seventeenth).
fn domains_share_by_weight() {
    let mut host = Host::new();
    let root = spawn(&mut host, "spin", &[]);
    parent(&mut host, root, 15);
    let mut many = alloc::vec![root];
    for _ in 0..15 {
        many.push(spinner_under(
            &mut host,
            root,
            child(32, 1 << 56, 0, u32::MAX),
        ));
    }
    let one = spawn(&mut host, "spin", &[]);
    parent(&mut host, one, 0);
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

/// Spec `cell-hierarchy`, "CPU weights between domains": on one CPU, two
/// spinning roots placed with weights 300 and 100 (protocol version 4,
/// `run --cpu-weight`) get three quarters and one quarter of it, within
/// 5 %, over 1 s.
fn root_weights_share_a_cpu() {
    let mut host = Host::new();
    let mut root = |weight| {
        let id = spawn(&mut host, "spin", &[]);
        parent(&mut host, id, 0);
        let spec = Spec {
            weight,
            ..Spec::default()
        };
        host.place(id, HOST, spec, 0).expect("placed");
        id
    };
    let (heavy, light) = (root(300), root(100));
    step_for(&mut host, 1_000_000_000);
    let (a, b) = (cpu_of(&mut host, &[heavy]), cpu_of(&mut host, &[light]));
    let pct = 100 * a / (a + b);
    assert!(
        (70..=80).contains(&pct),
        "weight 300 got {pct} % ({a} ns of {})",
        a + b
    );
    for id in [heavy, light] {
        assert!(host.kill(id));
    }
    assert!(host.tree().is_empty());
}

/// Spec `cell-hierarchy`, "Capped subtree": a subtree of two spinning
/// cells capped at 0.25 CPU (under an idle root), alone on idle CPUs, gets
/// at most 25 % of a CPU plus one slice per period for each CPU running it
/// (one with `ktest`, two with `ktest-smp`), on every CPU at once. The cap
/// is charged at the end of a slice, which lasts up to two timer ticks of
/// [`SLICE_NS`] (a cell is preempted at its second).
fn cpu_cap_holds() {
    const PERIOD: u64 = rhizo_sched::fair::PERIOD_NS;
    let mut host = Host::new();
    let root = spawn(&mut host, "idle", &[]);
    parent(&mut host, root, 2);
    let top = spinner_under(&mut host, root, child(256, 1 << 60, 1, 250));
    let capped = [
        top,
        spinner_under(&mut host, top, child(64, 1 << 58, 0, 250)),
    ];
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
    for id in [capped[1], capped[0], root] {
        assert!(host.kill(id));
    }
    assert!(host.tree().is_empty());
}
