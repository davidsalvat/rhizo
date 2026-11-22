//! How cells share the CPUs: by weight between domains, within caps per
//! subtree (design D23 of `add-cell-hierarchy`), and where they run when
//! domains have exclusive CPU sets (design D17).

use super::*;
use crate::cell::now_ns;
use rhizo_cap::{Actor, Error as TreeError, HOST, Limits, Spec};
use rhizo_cell::quota::Quota;
use rhizo_sched::SLICE_NS;

/// This area's tests, in execution order.
pub(super) const ALL: &[TestCase] = &[
    ktest_case!(domains_share_by_weight, 45),
    ktest_case!(root_weights_share_a_cpu, 25),
    ktest_case!(cpu_cap_holds, 25),
    ktest_case!(exclusive_sets_hold, 25),
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

/// One CPU stepping through every cell for at least `ns`, and until the
/// CPU time the scheduler charged the domains of cells `x` and `y` is 20
/// times the longest slice charged (the most it moves in one step): their
/// charges and that slice. Within one slice of its weight's part each
/// (design D23), a domain's share is then within 5 % of it.
fn charged_until(host: &mut Host, x: u32, y: u32, ns: u64) -> (u64, u64, u64) {
    let charged = |host: &mut Host, id| host.charged_ns(id).expect("shared by weight");
    let (t0, mut longest, mut before) = (now_ns(), 0, 0);
    loop {
        host.step();
        let (a, b) = (charged(host, x), charged(host, y));
        longest = longest.max(a + b - before);
        before = a + b;
        if now_ns() - t0 >= ns && a + b >= 20 * longest {
            return (a, b, longest);
        }
    }
}

/// Spec `cell-hierarchy`, "Many cells do not buy CPU": on one CPU, a
/// domain of 16 spinning cells (a root and 15 children) and a domain of
/// one, with equal weights, each get half the CPU time the scheduler
/// charges, within 5 % (round robin per cell would give the one cell a
/// seventeenth), over at least 2 s and 20 of the longest slice charged
/// (change `fix-fair-bound`): each domain is within one slice of its
/// half (design D23), however long a busy host stretched the slices.
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
    let (a, b, longest) = charged_until(&mut host, root, one, 2_000_000_000);
    let pct = 100 * b / (a + b);
    assert!(
        (45..=55).contains(&pct),
        "the one cell's domain got {pct} % ({b} ns of {}, longest slice {longest} ns)",
        a + b
    );
    for id in many.into_iter().rev().chain([one]) {
        assert!(host.kill(id));
    }
    assert!(host.tree().is_empty());
}

/// Spec `cell-hierarchy`, "CPU weights between domains": on one CPU, two
/// spinning roots placed with weights 300 and 100 (protocol version 4,
/// `run --cpu-weight`) get three quarters and one quarter of the CPU time
/// the scheduler charges, within 5 %, over at least 1 s and 20 of the
/// longest slice charged (change `fix-share-test`). The scheduler keeps
/// each domain within one slice of its weight's part (design D23), so 20
/// slices bound the share to 5 %, however long a busy host stretched them.
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
    let (a, b, longest) = charged_until(&mut host, heavy, light, 1_000_000_000);
    let pct = 100 * a / (a + b);
    assert!(
        (70..=80).contains(&pct),
        "weight 300 got {pct} % ({a} ns of {}, longest slice {longest} ns)",
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

/// Spec `isolation`, "Exclusive CPU sets per domain" (design D17),
/// hostile: a domain given the upper half of the CPUs (2 and 3 of 4) runs
/// only there, and no cell of another domain runs there, over a busy run
/// with stealing and balancing: the domain's three cells, one spreading
/// three threads, on its two CPUs; two spinning roots per CPU, one of
/// weight 300 (cells share by weight too), and a root spreading a thread
/// per CPU, on the others, which serve them all. Checked by the CPUs each
/// cell ran on. Sets holding the boot CPU or CPUs the machine lacks, or
/// leaving no CPU shared beside it, a set overlapping another root's and a
/// set for a child are refused (on fewer than three CPUs, every set).
fn exclusive_sets_hold() {
    let cpus = crate::smp::count();
    let all = (1u64 << cpus) - 1;
    let mut host = Host::new();
    let with = |set: u64, cells: u32| Spec {
        cpus: set,
        limits: Limits {
            cells,
            ..Limits::UNLIMITED
        },
        ..Spec::default()
    };
    let probe = spawn(&mut host, "idle", &[]);
    for bad in [1, 0b11, 1 << cpus, all, all & !1]
        .into_iter()
        .filter(|&b| b != 0)
    {
        let refused = host.place(probe, HOST, with(bad, 0), 0);
        assert_eq!(refused, Err(TreeError::Invalid), "set {bad:#b}");
    }
    if cpus < 3 {
        assert!(host.kill(probe));
        assert!(host.tree().is_empty());
        return;
    }
    let set = all & !(all >> (cpus / 2)); // CPUs 2 and 3 of 4
    let shared = all & !set;
    // The domain with the set: a spinning root, a spinning child and a
    // child spreading three threads.
    let root = spawn(&mut host, "spin", &[]);
    parent(&mut host, root, 2);
    host.place(root, HOST, with(set, 2), 0).expect("its set");
    let threads = spawn(&mut host, "threads", &[b"spin", b"3"]);
    let child_spec = child(32, 1 << 56, 0, u32::MAX);
    host.adopt(threads, root, child_spec.clone(), Actor::Host)
        .expect("placed");
    let inside = [root, spinner_under(&mut host, root, child_spec), threads];
    let low = set & set.wrapping_neg();
    let overlap = host.place(probe, HOST, with(low, 0), 0);
    assert_eq!(overlap, Err(TreeError::Invalid), "another root's CPU");
    let to_child = host.adopt(probe, root, with(low, 0), Actor::Host);
    assert_eq!(to_child, Err(TreeError::Invalid), "a child's set");
    assert!(host.kill(probe));
    // Every other domain, hostile: more cells than the shared CPUs, one
    // heavier, one spreading over every CPU.
    let mut outside: Vec<u32> = (0..2 * cpus)
        .map(|_| spawn(&mut host, "spin", &[]))
        .collect();
    let heavy = Spec {
        weight: 300,
        ..Spec::default()
    };
    host.place(outside[0], HOST, heavy, 0).expect("weight");
    let spread = alloc::format!("{cpus}");
    outside.push(spawn(&mut host, "threads", &[b"spin", spread.as_bytes()]));
    host.run_until(now_ns() + 1_000_000_000, |_| false);
    for &id in inside.iter().rev().chain(&outside) {
        assert!(host.kill(id), "cell {id} still ran");
    }
    let ran = |ids: &[u32]| -> Vec<u64> {
        let on = |id: u32| u64::from(host.finished(id).expect("ended").cpus);
        ids.iter().map(|&id| on(id)).collect()
    };
    let (mine, theirs) = (ran(&inside), ran(&outside));
    for (id, on) in inside.iter().zip(&mine) {
        assert!(
            *on != 0 && on & !set == 0,
            "cell {id} of the domain with CPUs {set:#b} ran on {on:#b}"
        );
    }
    for (id, on) in outside.iter().zip(&theirs) {
        assert!(
            *on != 0 && on & set == 0,
            "cell {id} of another domain ran on {on:#b}, inside the set {set:#b}"
        );
    }
    let used = |r: &[u64]| r.iter().fold(0, |m, on| m | on);
    assert_eq!(used(&mine), set, "the domain ran on each of its CPUs");
    assert_eq!(used(&theirs), shared, "the shared CPUs served the others");
    assert!(host.tree().is_empty());
}
