use crate::limits::{DEPTH_MAX, RULES_MAX};
use crate::*;
use proptest::prelude::*;

const MIB: u64 = 1 << 20;

fn limits(memory: u64, cells: u32) -> Limits {
    Limits {
        memory,
        cpu_ns: u64::MAX,
        sockets: (memory / MIB) as u32, // one per MiB
        cells,
    }
}

fn timed(memory: u64, cpu_ns: u64, cells: u32) -> Limits {
    Limits {
        cpu_ns,
        ..limits(memory, cells)
    }
}

fn spec(l: Limits) -> Spec {
    Spec {
        limits: l,
        ..Spec::default()
    }
}

fn none(_: u32) -> Limits {
    Limits::ZERO
}

/// A source of fresh domain identifiers from 1000 up.
fn fresh() -> impl FnMut() -> u32 {
    let mut next = 1000;
    move || {
        next += 1;
        next
    }
}

fn rule(proto: Proto, addr: [u8; 4], prefix: u8, lo: u16, hi: u16) -> Rule {
    Rule::new(proto, addr, prefix, lo, hi).unwrap()
}

#[test]
fn roots_start_domains_numbered_from_one() {
    let mut t = Tree::new();
    t.add_root(5, spec(limits(64 * MIB, 8)), 0).unwrap();
    t.add_root(9, Spec::default(), 0).unwrap();
    t.add_child(12, 5, spec(limits(16 * MIB, 0)), Actor::Host, &none)
        .unwrap();
    t.add_child(13, 5, spec(limits(16 * MIB, 0)), Actor::Host, &none)
        .unwrap();
    let n = |id| t.get(id).unwrap();
    assert_eq!(
        (n(5).pid(), n(9).pid(), n(12).pid(), n(13).pid()),
        (1, 1, 2, 3)
    );
    assert_eq!((n(12).domain(), n(9).domain()), (5, 9));
    assert_eq!(t.root_of(13), Some(5));
    assert_eq!(t.own(5).unwrap().memory, 32 * MIB);
    assert_eq!(t.own(5).unwrap().cells, 6);
    assert_eq!(t.children(5).collect::<Vec<_>>(), [13, 12]);
    assert_eq!(t.children(HOST).collect::<Vec<_>>(), [5, 9]);
    assert_eq!(n(12).egress(), &Egress::None, "egress none by default");
    t.check().unwrap();
}

#[test]
fn children_need_a_bounded_parent_and_room() {
    let mut t = Tree::new();
    t.add_root(1, Spec::default(), 0).unwrap();
    let child = spec(limits(MIB, 0));
    assert_eq!(
        t.add_child(2, 1, child.clone(), Actor::Host, &none),
        Err(Error::Unlimited)
    );
    t.set_limits(1, limits(4 * MIB, 2), Actor::Host, &none)
        .unwrap();
    // The parent uses 3 MiB itself: one more MiB is free.
    let uses = |id: u32| Limits {
        memory: if id == 1 { 3 * MIB } else { 0 },
        ..Limits::ZERO
    };
    let two = spec(limits(2 * MIB, 0));
    assert_eq!(
        t.add_child(2, 1, two, Actor::Host, &uses),
        Err(Error::Budget)
    );
    t.add_child(2, 1, child.clone(), Actor::Host, &uses)
        .unwrap();
    t.add_child(3, 1, child.clone(), Actor::Host, &none)
        .unwrap();
    assert_eq!(
        t.add_child(4, 1, child, Actor::Host, &none),
        Err(Error::Budget),
        "two descendants"
    );
    t.check().unwrap();
}

/// Spec `cell-hierarchy`, "Fork bomb": a cell with a limit of 8
/// descendants never has more in its subtree, however its children try.
#[test]
fn fork_bomb_stops_at_the_descendant_limit() {
    let mut t = Tree::new();
    t.add_root(1, spec(limits(64 * MIB, 8)), 0).unwrap();
    let mut next = 2;
    let mut made = 0;
    for _ in 0..100 {
        let parents: Vec<u32> = t.iter().map(|n| n.id()).collect();
        for p in parents {
            let own = t.own(p).unwrap();
            let want = Limits {
                cells: own.cells.saturating_sub(1),
                memory: own.memory / 2,
                ..own
            };
            if t.add_child(next, p, spec(want), Actor::Cell(p), &none)
                .is_ok()
            {
                made += 1;
            }
            next += 1;
        }
    }
    assert_eq!(made, 8);
    assert_eq!(t.subtree(1).unwrap().len(), 9);
    t.check().unwrap();
}

#[test]
fn depth_is_bounded() {
    let mut t = Tree::new();
    t.add_root(1, spec(limits(64 * MIB, 100)), 0).unwrap();
    let mut parent = 1;
    for id in 2..=u32::from(DEPTH_MAX) {
        let own = t.own(parent).unwrap();
        let l = limits(own.memory, own.cells - 1);
        t.add_child(id, parent, spec(l), Actor::Cell(parent), &none)
            .unwrap();
        parent = id;
    }
    assert_eq!(t.get(parent).unwrap().depth(), DEPTH_MAX);
    let r = t.add_child(99, parent, spec(limits(0, 0)), Actor::Cell(parent), &none);
    assert_eq!(r, Err(Error::TooDeep));
    t.check().unwrap();
}

/// Spec `cell-hierarchy`, "A child turns on its parent" and "Moves that
/// are refused".
#[test]
fn authority_flows_down() {
    let mut t = Tree::new();
    let mut f = fresh();
    t.add_root(1, spec(limits(64 * MIB, 8)), 0).unwrap();
    t.add_child(2, 1, spec(limits(8 * MIB, 2)), Actor::Host, &none)
        .unwrap();
    t.add_child(3, 1, spec(limits(8 * MIB, 0)), Actor::Host, &none)
        .unwrap();
    t.add_child(4, 2, spec(limits(MIB, 0)), Actor::Host, &none)
        .unwrap();
    t.add_root(10, spec(limits(64 * MIB, 8)), 0).unwrap();
    let before = t.clone();
    assert!(t.controls(Actor::Cell(1), 4) && t.controls(Actor::Cell(2), 4));
    assert!(!t.controls(Actor::Cell(2), 2) && !t.controls(Actor::Cell(2), 1));
    assert!(!t.controls(Actor::Cell(2), 3), "a sibling");
    let r = |t: &mut Tree, id, to, by, f: &mut dyn FnMut() -> u32| t.reparent(id, to, by, &none, f);
    assert_eq!(
        r(&mut t, 2, HOST, Actor::Cell(2), &mut f),
        Err(Error::Denied),
        "itself"
    );
    assert_eq!(
        r(&mut t, 1, HOST, Actor::Cell(2), &mut f),
        Err(Error::Denied),
        "its parent"
    );
    assert_eq!(
        r(&mut t, 3, 2, Actor::Cell(2), &mut f),
        Err(Error::Denied),
        "a sibling"
    );
    assert_eq!(
        r(&mut t, 2, 4, Actor::Host, &mut f),
        Err(Error::Cycle),
        "under its child"
    );
    assert_eq!(
        r(&mut t, 2, 10, Actor::Cell(1), &mut f),
        Err(Error::Denied),
        "another domain"
    );
    assert_eq!(
        t.set_limits(2, limits(MIB, 0), Actor::Cell(2), &none),
        Err(Error::Denied)
    );
    assert_eq!(
        t.set_egress(1, Egress::Full, Egress::Full, Actor::Cell(2)),
        Err(Error::Denied)
    );
    assert_eq!(t, before);
    // The parent moves its grandchild under its other child.
    r(&mut t, 4, 3, Actor::Cell(1), &mut f).unwrap_err(); // 3 has no descendants left
    t.set_limits(3, limits(8 * MIB, 1), Actor::Cell(1), &none)
        .unwrap();
    r(&mut t, 4, 3, Actor::Cell(1), &mut f).unwrap();
    assert_eq!(t.get(4).unwrap().parent(), 3);
    assert_eq!(t.reserved(2), Some(Limits::ZERO));
    t.check().unwrap();
}

/// Spec `cell-hierarchy`, "Parent promotes a child".
#[test]
fn promotion_moves_the_reservation() {
    let mut t = Tree::new();
    let mut f = fresh();
    t.add_root(1, spec(limits(64 * MIB, 4)), 0).unwrap();
    t.add_child(2, 1, spec(limits(16 * MIB, 1)), Actor::Host, &none)
        .unwrap();
    t.add_child(3, 2, spec(limits(4 * MIB, 0)), Actor::Host, &none)
        .unwrap();
    t.reparent(2, HOST, Actor::Cell(1), &none, &mut f).unwrap();
    let (p, c) = (t.get(1).unwrap(), t.get(2).unwrap());
    assert_eq!(p.limits().memory, 48 * MIB);
    assert_eq!(p.limits().cells, 2);
    assert_eq!(t.reserved(1), Some(Limits::ZERO));
    assert!(c.is_root());
    assert_eq!(
        (c.limits().memory, c.domain(), c.pid()),
        (16 * MIB, 1001, 1)
    );
    let g = t.get(3).unwrap();
    assert_eq!((g.domain(), g.pid(), g.depth()), (1001, 2, 2));
    t.check().unwrap();
}

#[test]
fn the_host_promotes_without_shrinking() {
    let mut t = Tree::new();
    t.add_root(1, spec(limits(64 * MIB, 4)), 0).unwrap();
    t.add_child(2, 1, spec(limits(16 * MIB, 0)), Actor::Host, &none)
        .unwrap();
    t.reparent(2, HOST, Actor::Host, &none, &mut fresh())
        .unwrap();
    assert_eq!(t.get(1).unwrap().limits().memory, 64 * MIB);
    assert_eq!(t.own(1).unwrap().memory, 64 * MIB);
    t.check().unwrap();
}

/// Spec `cell-hierarchy`, "Cascade" and "Promote".
#[test]
fn ending_cascades_or_promotes() {
    let mut t = Tree::new();
    let child = |t: &mut Tree, id, parent, l| t.add_child(id, parent, spec(l), Actor::Host, &none);
    t.add_root(1, spec(timed(64 * MIB, 1_000_000, 8)), 0)
        .unwrap();
    child(&mut t, 2, 1, timed(32 * MIB, 100_000, 6)).unwrap();
    child(&mut t, 3, 2, timed(4 * MIB, 10_000, 1)).unwrap();
    child(&mut t, 4, 3, timed(MIB, 1000, 0)).unwrap();
    let keep = Spec {
        on_parent_end: OnParentEnd::Promote,
        ..spec(timed(8 * MIB, 10_000, 1))
    };
    t.add_child(5, 2, keep, Actor::Host, &none).unwrap();
    child(&mut t, 6, 5, timed(MIB, 1000, 0)).unwrap();
    let ended = t.end(2, &mut fresh()).unwrap();
    assert_eq!(ended.gone, [4, 3]);
    assert_eq!(ended.promoted, [5]);
    let root = t.get(1).unwrap();
    assert_eq!(
        root.limits().memory,
        56 * MIB,
        "the promoted reservation left"
    );
    assert_eq!(root.limits().cells, 6);
    assert_eq!(root.limits().cpu_ns, 1_000_000 - 10_000);
    assert_eq!(t.reserved(1), Some(Limits::ZERO));
    let p = t.get(5).unwrap();
    assert!(p.is_root() && p.pid() == 1 && t.get(6).unwrap().pid() == 2);
    assert_eq!(t.len(), 3);
    t.check().unwrap();
    assert_eq!(t.end(2, &mut fresh()), Err(Error::NotFound));
}

#[test]
fn copies_are_siblings_with_new_numbers() {
    let mut t = Tree::new();
    t.add_root(1, spec(limits(64 * MIB, 8)), 0).unwrap();
    t.add_child(2, 1, spec(limits(16 * MIB, 0)), Actor::Host, &none)
        .unwrap();
    assert_eq!(
        t.add_copies(2, &[3, 4, 5, 6], &none, &mut |_, _, _| {}),
        Err(Error::Budget)
    );
    t.add_copies(2, &[3, 4, 5], &none, &mut |_, _, _| {})
        .unwrap();
    assert!((3..=5).all(|c| t.get(c).unwrap().parent() == 1));
    assert_eq!(t.get(5).unwrap().pid(), 5);
    assert_eq!(t.own(1).unwrap().memory, 0);
    // Copies of a root are roots of new domains; none takes its number.
    let mut seen = Vec::new();
    t.add_copies(1, &[7, 8], &none, &mut |k, pid, domain| {
        seen.push((k, pid, domain))
    })
    .unwrap();
    let (a, b) = (t.get(7).unwrap(), t.get(8).unwrap());
    assert!(a.is_root() && a.domain() != b.domain() && a.domain() != 1);
    assert_eq!((a.pid(), b.pid(), a.children()), (2, 2, 0));
    assert_eq!(seen, [(0, 2, a.domain()), (1, 2, b.domain())]);
    assert_eq!(
        t.add_copies(1, &[9, 9], &none, &mut |_, _, _| {}),
        Err(Error::Exists)
    );
    t.check().unwrap();
}

#[test]
fn rekey_keeps_the_place() {
    let mut t = Tree::new();
    t.add_root(1, spec(limits(64 * MIB, 8)), 0).unwrap();
    t.add_child(2, 1, spec(limits(16 * MIB, 2)), Actor::Host, &none)
        .unwrap();
    t.add_child(3, 1, spec(limits(16 * MIB, 0)), Actor::Host, &none)
        .unwrap();
    t.add_child(4, 2, spec(limits(MIB, 0)), Actor::Host, &none)
        .unwrap();
    assert!(t.rekey(2, 40).is_some());
    assert!(t.rekey(2, 41).is_none() && t.rekey(3, 1).is_none());
    let n = t.get(40).unwrap();
    assert_eq!((n.parent(), n.pid(), n.domain()), (1, 2, 1));
    assert_eq!(t.get(4).unwrap().parent(), 40);
    assert_eq!(t.rekey(1, 50).map(Node::pid), Some(1));
    assert_eq!(t.get(40).unwrap().parent(), 50);
    assert_eq!(t.domain(4), Some(1), "the domain keeps its identifier");
    t.check().unwrap();
}

#[test]
fn moves_across_domains_are_the_hosts() {
    let mut t = Tree::new();
    t.add_root(1, spec(limits(64 * MIB, 8)), 0).unwrap();
    t.add_root(2, spec(limits(64 * MIB, 8)), 0).unwrap();
    t.add_child(3, 2, spec(limits(8 * MIB, 0)), Actor::Host, &none)
        .unwrap();
    let full = Spec {
        egress: Egress::Full,
        ..spec(limits(8 * MIB, 0))
    };
    assert_eq!(
        t.add_child(4, 1, full, Actor::Host, &none),
        Err(Error::Denied),
        "wider egress"
    );
    t.set_egress(2, Egress::Full, Egress::Full, Actor::Host)
        .unwrap();
    t.set_egress(3, Egress::Full, Egress::Full, Actor::Host)
        .unwrap();
    t.reparent(3, 1, Actor::Host, &none, &mut fresh()).unwrap();
    let n = t.get(3).unwrap();
    assert_eq!((n.domain(), n.pid(), n.egress()), (1, 2, &Egress::None));
    assert_eq!(t.reserved(2), Some(Limits::ZERO));
    t.check().unwrap();
}

#[test]
fn labels_and_weights_are_checked() {
    assert!(Label::new(b"tools.v1_a-b").is_ok());
    assert_eq!(Label::new(b"a b"), Err(Error::Invalid));
    assert_eq!(Label::new(&[b'a'; 65]), Err(Error::Invalid));
    let mut t = Tree::new();
    let heavy = Spec {
        weight: 10_001,
        ..Spec::default()
    };
    assert_eq!(t.add_root(1, heavy, 0), Err(Error::Invalid));
    assert_eq!(t.add_root(HOST, Spec::default(), 0), Err(Error::Invalid));
    t.add_root(1, spec(limits(MIB, 1)), 0).unwrap();
    let set = Spec {
        cpus: 0b1100,
        ..spec(limits(0, 0))
    };
    assert_eq!(
        t.add_child(2, 1, set, Actor::Host, &none),
        Err(Error::Invalid)
    );
}

#[test]
fn rules_order_and_intersect() {
    let web = rule(Proto::Tcp, [140, 82, 112, 0], 20, 443, 443);
    let any = rule(Proto::Any, [140, 82, 0, 0], 16, 0, u16::MAX);
    let wide = Egress::rules(&[any]).unwrap();
    let narrow = Egress::rules(&[web]).unwrap();
    assert!(narrow.within(&wide) && !wide.within(&narrow));
    assert!(Egress::None.within(&narrow) && narrow.within(&Egress::Full));
    assert!(!Egress::Full.within(&wide));
    assert_eq!(wide.intersect(&narrow).unwrap(), narrow);
    assert_eq!(Egress::Full.intersect(&wide).unwrap(), wide);
    assert!(narrow.allows(Proto::Tcp, [140, 82, 121, 4], 443));
    assert!(!narrow.allows(Proto::Udp, [140, 82, 121, 4], 443));
    assert!(!narrow.allows(Proto::Tcp, [140, 82, 128, 4], 443));
    let udp = Egress::rules(&[rule(Proto::Udp, [0; 4], 0, 53, 53)]).unwrap();
    assert_eq!(udp.intersect(&narrow).unwrap(), Egress::None);
    assert_eq!(Rule::new(Proto::Tcp, [0; 4], 33, 1, 1), None);
    assert_eq!(Rule::new(Proto::Tcp, [0; 4], 8, 2, 1), None);
    let too_many = vec![web; RULES_MAX + 1];
    assert_eq!(Egress::rules(&too_many), Err(Error::Invalid));
}

// Property tests.

fn arb_rule() -> impl Strategy<Value = Rule> {
    (
        prop_oneof![Just(Proto::Tcp), Just(Proto::Udp), Just(Proto::Any)],
        // Few networks, so that rules often meet.
        (0u8..4, 0u8..4),
        prop_oneof![Just(0u8), Just(8), Just(16), Just(24), Just(32)],
        (0u16..8, 0u16..8),
    )
        .prop_map(|(proto, (a, b), prefix, (p, q))| {
            rule(proto, [10, a, b, 1], prefix, p.min(q) * 100, p.max(q) * 100)
        })
}

fn arb_egress() -> impl Strategy<Value = Egress> {
    prop_oneof![
        Just(Egress::None),
        Just(Egress::Full),
        prop::collection::vec(arb_rule(), 1..4).prop_map(|r| Egress::rules(&r).unwrap()),
    ]
}

proptest! {
    /// Narrowing is transitive, the intersection is within both policies,
    /// and a policy within another allows nothing the other does not.
    #[test]
    fn narrowing_is_transitive(
        a in arb_egress(),
        b in arb_egress(),
        c in arb_egress(),
        probes in prop::collection::vec((any::<bool>(), 0u8..4, 0u8..4, 0u16..800), 16),
    ) {
        if a.within(&b) && b.within(&c) {
            prop_assert!(a.within(&c));
        }
        let ab = a.intersect(&b).unwrap();
        prop_assert!(ab.within(&a) && ab.within(&b));
        prop_assert!(a.within(&a));
        for (tcp, x, y, port) in probes {
            let proto = if tcp { Proto::Tcp } else { Proto::Udp };
            let addr = [10, x, y, 1];
            if a.within(&b) && a.allows(proto, addr, port) {
                prop_assert!(b.allows(proto, addr, port));
            }
            prop_assert_eq!(
                ab.allows(proto, addr, port),
                a.allows(proto, addr, port) && b.allows(proto, addr, port)
            );
        }
    }
}

#[derive(Debug, Clone)]
enum Op {
    Root(Limits),
    /// A child of the first cell, asked by the second (`None`: the host),
    /// taking eighths of the parent's free budget, with a policy within the
    /// parent's or not.
    Child(usize, Option<usize>, u8, bool, Egress, bool),
    Copies(usize, u8),
    Reparent(usize, Option<usize>, Option<usize>),
    End(usize),
    SetLimits(usize, Option<usize>, Limits),
    SetEgress(usize, Option<usize>, Egress),
    Rekey(usize),
}

fn arb_limits() -> impl Strategy<Value = Limits> {
    (
        prop_oneof![1 => Just(u64::MAX), 4 => (0u64..64).prop_map(|m| m * MIB)],
        prop_oneof![1 => Just(u64::MAX), 4 => 0u64..10_000],
        prop_oneof![1 => Just(u32::MAX), 4 => 0u32..64],
        prop_oneof![1 => Just(u32::MAX), 4 => 0u32..12],
    )
        .prop_map(|(memory, cpu_ns, sockets, cells)| Limits {
            memory,
            cpu_ns,
            sockets,
            cells,
        })
}

fn arb_op() -> impl Strategy<Value = Op> {
    let i = || 0usize..64;
    prop_oneof![
        1 => arb_limits().prop_map(Op::Root),
        6 => (i(), prop::option::of(i()), 0u8..=8, any::<bool>(), arb_egress(), prop::bool::weighted(0.8))
            .prop_map(|(p, by, f, promote, e, within)| Op::Child(p, by, f, promote, e, within)),
        1 => (i(), 1u8..4).prop_map(|(s, n)| Op::Copies(s, n)),
        2 => (i(), prop::option::of(i()), prop::option::of(i()))
            .prop_map(|(c, to, by)| Op::Reparent(c, to, by)),
        2 => i().prop_map(Op::End),
        1 => (i(), prop::option::of(i()), arb_limits()).prop_map(|(c, by, l)| Op::SetLimits(c, by, l)),
        1 => (i(), prop::option::of(i()), arb_egress()).prop_map(|(c, by, e)| Op::SetEgress(c, by, e)),
        1 => i().prop_map(Op::Rekey),
    ]
}

/// What a cell uses itself in the property tests: a little, by identifier.
fn usage(id: u32) -> Limits {
    Limits {
        memory: u64::from(id % 3) * MIB,
        cpu_ns: u64::from(id % 5),
        sockets: id % 2,
        cells: 0,
    }
}

/// Runs `op` on `t`, with identifiers from `next`.
fn apply(t: &mut Tree, op: &Op, next: &mut u32, domains: &mut u32) -> Result<(), Error> {
    let ids: Vec<u32> = t.iter().map(|n| n.id()).collect();
    let pick = |i: usize| ids.get(i % ids.len().max(1)).copied().unwrap_or(1);
    let actor = |i: Option<usize>| i.map_or(Actor::Host, |i| Actor::Cell(pick(i)));
    let mut fresh = || {
        *domains += 1;
        *domains
    };
    *next += 4;
    let id = *next;
    match op {
        Op::Root(l) => t.add_root(id, spec(*l), 0).map(|_| ()),
        Op::Child(p, by, f, promote, e, within) => {
            let parent = pick(*p);
            let Some(n) = t.get(parent) else {
                return Err(Error::NotFound);
            };
            let free = t.free_budget(parent, usage(parent)).unwrap();
            let part = |x: u64| {
                if x == u64::MAX {
                    x
                } else {
                    x / 8 * u64::from(*f)
                }
            };
            let l = Limits {
                memory: part(free.memory),
                cpu_ns: part(free.cpu_ns),
                sockets: part(u64::from(free.sockets)) as u32,
                cells: (part(u64::from(free.cells)) as u32).saturating_sub(1),
            };
            let egress = if *within {
                e.intersect(n.egress())?
            } else {
                e.clone()
            };
            let s = Spec {
                on_parent_end: if *promote {
                    OnParentEnd::Promote
                } else {
                    OnParentEnd::Cascade
                },
                egress,
                ..spec(l)
            };
            t.add_child(id, parent, s, actor(*by), &usage)
        }
        Op::Copies(s, n) => {
            let ids: Vec<u32> = (0..u32::from(*n)).map(|k| id + k).collect();
            t.add_copies(pick(*s), &ids, &usage, &mut |_, _, _| {})
        }
        Op::Reparent(c, to, by) => {
            let to = to.map_or(HOST, pick);
            t.reparent(pick(*c), to, actor(*by), &usage, &mut fresh)
        }
        Op::End(c) => t.end(pick(*c), &mut fresh).map(|_| ()),
        Op::SetLimits(c, by, l) => t.set_limits(pick(*c), *l, actor(*by), &usage),
        Op::SetEgress(c, by, e) => t.set_egress(pick(*c), e.clone(), e.clone(), actor(*by)),
        Op::Rekey(c) => {
            t.rekey(pick(*c), id);
            Ok(())
        }
    }
}

/// The budget invariants beyond [`Tree::check`]: no subtree holds more
/// than its root's limits, summing what each of its cells may use itself.
fn budgets_hold(t: &Tree) {
    for root in t.iter().filter(|n| n.is_root()) {
        let sub = t.subtree(root.id()).unwrap();
        let held = sub
            .iter()
            .map(|&c| t.own(c).unwrap().memory)
            .fold(0u64, u64::saturating_add);
        if root.limits().memory != u64::MAX {
            assert!(
                held <= root.limits().memory,
                "a subtree holds more than its root"
            );
        }
        for &c in &sub {
            let (n, r) = (t.get(c).unwrap(), t.reserved(c).unwrap());
            assert!(r.memory <= n.limits().memory && r.cells <= n.limits().cells);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 200, ..ProptestConfig::default() })]

    /// Budgets never go negative or over the root, and every refused
    /// operation leaves the tree as it was.
    #[test]
    fn budgets_hold_under_any_operations(ops in prop::collection::vec(arb_op(), 1..60)) {
        let mut t = Tree::new();
        t.add_root(1, spec(timed(64 * MIB, 100_000, 32)), 0).unwrap();
        let (mut next, mut domains) = (1, 1);
        for op in &ops {
            let before = t.clone();
            if apply(&mut t, op, &mut next, &mut domains).is_err() {
                prop_assert_eq!(&t, &before, "{:?} failed but changed the tree", op);
            }
            prop_assert_eq!(t.check(), Ok(()), "after {:?}", op);
            budgets_hold(&t);
        }
    }

    /// Every operation with any one of its allocations made to fail
    /// reports it and leaves the tree unchanged.
    #[test]
    fn failed_allocations_change_nothing(ops in prop::collection::vec(arb_op(), 1..40)) {
        let mut t = Tree::new();
        t.add_root(1, spec(timed(64 * MIB, 100_000, 32)), 0).unwrap();
        let (mut next, mut domains) = (1, 1);
        for op in &ops {
            for k in 0.. {
                let before = t.clone();
                let (n, d) = (next, domains);
                FAIL_IN.with(|f| f.set(Some(k)));
                let r = apply(&mut t, op, &mut next, &mut domains);
                FAIL_IN.with(|f| f.set(None));
                let failed = FAILED.with(|f| f.replace(false));
                if failed && r.is_err() {
                    prop_assert_eq!(r, Err(Error::NoMemory));
                    prop_assert_eq!(&t, &before, "{:?} changed the tree", op);
                    (next, domains) = (n, d);
                    continue;
                }
                prop_assert_eq!(t.check(), Ok(()));
                break;
            }
        }
    }
}
