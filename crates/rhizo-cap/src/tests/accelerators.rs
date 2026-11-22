//! Accelerator handles (design D24): rights, models and budgets narrowed by
//! derivation, carved from the source and admitted against what the host
//! registered; no inheritance by copies.

use super::grants::rights_of;
use super::{MIB, limits, spec};
use crate::*;
use proptest::prelude::*;

const GIB: u64 = 1 << 30;

fn none(_: u32) -> Limits {
    Limits::ZERO
}

fn no_use(_: u32) -> AccelBudget {
    AccelBudget::ZERO
}

fn budget(mem: u64, sessions: u32) -> AccelBudget {
    AccelBudget {
        mem,
        kv_host: mem / 4,
        time: 100 * sessions,
        sessions,
    }
}

fn accel(rights: Rights, holder: u32, models: u64, b: AccelBudget) -> Ask {
    Ask {
        rights,
        holder,
        label: Label::default(),
        accel: AccelTerms { models, budget: b },
    }
}

/// A root 1 with children 2 and 3, and accelerator 7 serving models 0 and
/// 1 with 8 GiB and 8 sessions.
fn setup() -> (Tree, Channels, Grants) {
    let mut t = Tree::new();
    t.add_root(1, spec(limits(64 * MIB, 16)), 0).unwrap();
    for id in [2, 3] {
        t.add_child(id, 1, spec(limits(MIB, 2)), Actor::Host, &none)
            .unwrap();
    }
    let mut g = Grants::new();
    let terms = AccelTerms {
        models: 0b11,
        budget: budget(8 * GIB, 8),
    };
    g.register_accel(7, terms).unwrap();
    assert_eq!(g.register_accel(7, terms), Err(Error::Exists));
    (t, Channels::new(), g)
}

/// Spec `cell-hierarchy`, "Asking for more" (accelerators): a handle with
/// `infer` on one model yields neither `compute` nor another model.
#[test]
fn accelerators_narrow_only() {
    let (t, ch, mut g) = setup();
    let root = g
        .grant(
            &t,
            &ch,
            Actor::Host,
            Object::Accel(7),
            accel(Rights::INFER, 1, 0b01, budget(4 * GIB, 4)),
        )
        .unwrap();
    let before = g.clone();
    let more = [
        accel(Rights::COMPUTE, 2, 0b01, budget(GIB, 1)),
        accel(Rights::INFER, 2, 0b10, budget(GIB, 1)),
    ];
    for a in more {
        assert_eq!(g.derive(&t, 1, root, a, &no_use), Err(Error::Denied));
    }
    assert_eq!(g, before);
    let child = g
        .derive(
            &t,
            1,
            root,
            accel(Rights::INFER, 2, 0b01, budget(GIB, 1)),
            &no_use,
        )
        .unwrap();
    let c = g.get(child).unwrap();
    assert_eq!((c.rights(), c.accel().models), (Rights::INFER, 0b01));
    assert!(c.accel().within(g.get(root).unwrap().accel()));
    g.check(&t, &ch).unwrap();
}

/// Spec `cell-hierarchy`, "Carving a budget": budgets are carved from what
/// the source has free (less what its own sessions use) and come back
/// when the derived handle is revoked.
#[test]
fn accelerators_carve_budgets() {
    let (t, ch, mut g) = setup();
    let root = g
        .grant(
            &t,
            &ch,
            Actor::Host,
            Object::Accel(7),
            accel(Rights::INFER, 1, 0b11, budget(4 * GIB, 4)),
        )
        .unwrap();
    // One session of the root's own is open.
    let one_open = |id: u32| {
        if id == root {
            budget(0, 1)
        } else {
            AccelBudget::ZERO
        }
    };
    let two = accel(Rights::INFER, 2, 0b01, budget(GIB, 2));
    let a = g.derive(&t, 1, root, two.clone(), &one_open).unwrap();
    let before = g.clone();
    let more = Ask { holder: 3, ..two };
    assert_eq!(
        g.derive(&t, 1, root, more.clone(), &one_open),
        Err(Error::Budget)
    );
    assert_eq!(g, before, "a refusal changes nothing");
    // Carving further down takes from the child's budget, never more.
    assert_eq!(
        g.derive(
            &t,
            2,
            a,
            accel(Rights::INFER, 2, 0b01, budget(GIB, 3)),
            &no_use
        ),
        Err(Error::Budget)
    );
    g.revoke(&t, Actor::Host, a, &mut |_, _| {}).unwrap();
    g.derive(&t, 1, root, more, &one_open).unwrap();
    g.check(&t, &ch).unwrap();
}

/// The host's grants of an accelerator are admitted against what it
/// registered; cells only derive.
#[test]
fn accelerators_admitted_against_the_host() {
    let (t, ch, mut g) = setup();
    let h = Actor::Host;
    let big = accel(Rights::INFER, 1, 0b01, budget(6 * GIB, 6));
    g.grant(&t, &ch, h, Object::Accel(7), big.clone()).unwrap();
    assert_eq!(
        g.grant(&t, &ch, h, Object::Accel(7), big),
        Err(Error::Budget)
    );
    assert_eq!(
        g.grant(
            &t,
            &ch,
            h,
            Object::Accel(7),
            accel(Rights::INFER, 1, 0b100, budget(0, 0))
        ),
        Err(Error::Denied),
        "a model it does not serve"
    );
    assert_eq!(
        g.grant(
            &t,
            &ch,
            h,
            Object::Accel(8),
            accel(Rights::INFER, 1, 0, budget(0, 0))
        ),
        Err(Error::NotFound)
    );
    assert_eq!(
        g.grant(
            &t,
            &ch,
            Actor::Cell(1),
            Object::Accel(7),
            accel(Rights::INFER, 2, 0, budget(0, 0))
        ),
        Err(Error::Denied)
    );
    // Terms only on accelerators.
    let odd = accel(Rights::OBSERVE, 2, 1, budget(0, 0));
    assert_eq!(
        g.grant(&t, &ch, h, Object::Cell(3), odd),
        Err(Error::Invalid)
    );
    let mut gone = 0;
    g.unregister_accel(7, &mut |_, _| gone += 1);
    assert_eq!(gone, 1);
    g.check(&t, &ch).unwrap();
}

/// Spec `cell-hierarchy`, "Copy of a cell with a handle": a copy holds no
/// accelerator handle.
#[test]
fn accelerators_not_inherited_by_copies() {
    let (mut t, ch, mut g) = setup();
    let root = g
        .grant(
            &t,
            &ch,
            Actor::Host,
            Object::Accel(7),
            accel(Rights::INFER, 1, 0b01, budget(GIB, 2)),
        )
        .unwrap();
    g.derive(
        &t,
        1,
        root,
        accel(Rights::INFER, 2, 0b01, budget(GIB, 1)),
        &no_use,
    )
    .unwrap();
    t.add_copies(2, &[4], &none, &mut |_, _, _| {}).unwrap();
    g.inherit(&t, 2, &[4]).unwrap();
    assert!(g.iter().all(|(_, x)| x.holder() != 4));
    g.check(&t, &ch).unwrap();
}

#[derive(Debug, Clone)]
enum Op {
    Derive(usize, u16, usize),
    Revoke(usize),
    Unlink(usize),
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 200, ..ProptestConfig::default() })]

    /// Derived handles never exceed their source (rights, models, budget)
    /// and budgets never go negative, whatever is derived, revoked or let
    /// go.
    #[test]
    fn accelerators_never_exceed_their_source(
        ops in prop::collection::vec(prop_oneof![
            4 => (0usize..32, 0u16..0x4000, 0usize..3).prop_map(|(g, b, h)| Op::Derive(g, b, h)),
            1 => (0usize..32).prop_map(Op::Revoke),
            1 => (0usize..32).prop_map(Op::Unlink),
        ], 1..80),
    ) {
        let (t, ch, mut g) = setup();
        g.grant(&t, &ch, Actor::Host, Object::Accel(7), accel(Rights::INFER | Rights::STATS, 1, 0b11, budget(8 * GIB, 8)))
            .unwrap();
        // Each handle's own sessions use one session in three.
        let used = |id: u32| budget(0, u32::from(id.is_multiple_of(3)));
        for op in &ops {
            let ids: Vec<u32> = g.iter().map(|(id, _)| id).collect();
            let pick = |i: usize| ids[i % ids.len()];
            let before = g.clone();
            let r = match *op {
                Op::Derive(i, bits, h) => {
                    let from = pick(i);
                    let src = g.get(from).unwrap().clone();
                    let frac = |x: u64| x / 8 * u64::from(bits >> 10 & 15);
                    let a = accel(
                        rights_of(Object::Accel(7), bits),
                        [1, 2, 3][h],
                        u64::from(bits) & 0b111,
                        AccelBudget {
                            mem: frac(src.accel().budget.mem),
                            kv_host: frac(src.accel().budget.kv_host),
                            time: frac(u64::from(src.accel().budget.time)) as u32,
                            sessions: frac(u64::from(src.accel().budget.sessions)) as u32,
                        },
                    );
                    g.derive(&t, src.holder(), from, a, &used).map(|id| {
                        let new = g.get(id).unwrap();
                        assert!(src.rights().contains(new.rights()));
                        assert!(new.accel().within(src.accel()));
                    })
                }
                Op::Revoke(i) => g.revoke(&t, Actor::Host, pick(i), &mut |_, _| {}),
                Op::Unlink(i) => {
                    let id = pick(i);
                    let by = g.get(id).unwrap().holder();
                    g.unlink(by, id).map(|_| ())
                }
            };
            if r.is_err() {
                prop_assert_eq!(&g, &before);
            }
            prop_assert_eq!(g.check(&t, &ch), Ok(()), "after {:?}", op);
            if g.iter().count() == 0 {
                break;
            }
        }
    }
}
