//! The grant table: rights, derivation, transfer, revocation, control
//! flowing down and the exposure check (design D8, D9).

use super::{MIB, limits, rule, spec};
use crate::limits::{CHAIN_MAX, CHANNELS_MAX, HANDLES_MAX};
use crate::*;
use proptest::prelude::*;

fn no_use(_: u32) -> AccelBudget {
    AccelBudget::ZERO
}

fn none(_: u32) -> Limits {
    Limits::ZERO
}

fn ask(rights: Rights, holder: u32) -> Ask {
    Ask {
        rights,
        holder,
        ..Ask::default()
    }
}

/// A child of `parent` with egress `egress` and ceiling `ceiling`, and a
/// sixteenth of its parent's budget.
fn child(t: &mut Tree, id: u32, parent: u32, egress: Egress, ceiling: Egress) {
    let own = t.own(parent).unwrap();
    let s = Spec {
        egress,
        ceiling: Some(ceiling),
        ..spec(limits(own.memory / 16, (own.cells - 1) / 16))
    };
    t.add_child(id, parent, s, Actor::Host, &none).unwrap();
}

/// A root 1 with egress `full` and room for children.
fn root() -> Tree {
    let mut t = Tree::new();
    let s = Spec {
        egress: Egress::Full,
        ..spec(limits(1024 * MIB, 1024))
    };
    t.add_root(1, s, 0).unwrap();
    t
}

/// Every grant number `gone` is told.
fn collect(out: &mut Vec<u32>) -> impl FnMut(u32, &Grant) + '_ {
    move |id, _| out.push(id)
}

/// Spec `cell-hierarchy`, "Asking for more": no derivation yields a right
/// its source lacks.
#[test]
fn attenuation_never_adds_rights() {
    let mut t = root();
    child(&mut t, 2, 1, Egress::None, Egress::None);
    child(&mut t, 3, 1, Egress::None, Egress::None);
    let ch = Channels::new();
    let mut g = Grants::new();
    let observe = g
        .grant(
            &t,
            &ch,
            Actor::Cell(1),
            Object::Cell(3),
            ask(Rights::OBSERVE, 2),
        )
        .unwrap();
    let before = g.clone();
    assert_eq!(
        g.derive(&t, 2, observe, ask(Rights::CONTROL, 2), &no_use),
        Err(Error::Denied)
    );
    assert_eq!(
        g.derive(
            &t,
            2,
            observe,
            ask(Rights::OBSERVE | Rights::INPUT, 2),
            &no_use
        ),
        Err(Error::Denied)
    );
    assert_eq!(g, before, "refusals change nothing");
    let d = g
        .derive(&t, 2, observe, ask(Rights::OBSERVE, 2), &no_use)
        .unwrap();
    assert_eq!(g.get(d).unwrap().rights(), Rights::OBSERVE);
    assert_eq!(g.get(d).unwrap().parent(), Some(observe));
    assert_eq!(g.get(d).unwrap().grantor(), 2);
    // A right of another kind of object.
    assert_eq!(
        g.grant(&t, &ch, Actor::Host, Object::Cell(3), ask(Rights::SEND, 2)),
        Err(Error::Invalid)
    );
    g.check(&t, &ch).unwrap();
}

/// Spec `cell-hierarchy`, "Control handed to the controlled": a control
/// handle on a cell never reaches it or a descendant, whoever sends it.
#[test]
fn control_never_flows_up() {
    let mut t = root();
    child(&mut t, 2, 1, Egress::None, Egress::None);
    child(&mut t, 3, 2, Egress::None, Egress::None);
    let mut ch = Channels::new();
    let mut g = Grants::new();
    let control = g
        .grant(
            &t,
            &ch,
            Actor::Cell(1),
            Object::Cell(2),
            ask(Rights::CONTROL, 1),
        )
        .unwrap();
    for holder in [2, 3] {
        assert_eq!(
            g.derive(&t, 1, control, ask(Rights::CONTROL, holder), &no_use),
            Err(Error::Denied)
        );
        let host = ask(Rights::CONTROL, holder);
        assert_eq!(
            g.grant(&t, &ch, Actor::Host, Object::Cell(2), host),
            Err(Error::Denied)
        );
    }
    // Over a channel with the carry right: the send is refused.
    let c = ch.create(&t, Actor::Cell(1), &none).unwrap();
    let [e0, e1] = Channels::endpoints(c);
    let all = Rights::SEND | Rights::RECEIVE | Rights::CARRY;
    let mine = g
        .grant(&t, &ch, Actor::Cell(1), Object::Endpoint(e0), ask(all, 1))
        .unwrap();
    g.grant(&t, &ch, Actor::Cell(1), Object::Endpoint(e1), ask(all, 2))
        .unwrap();
    let before = g.clone();
    assert_eq!(
        g.transfer(&t, 1, control, mine, ask(Rights::CONTROL, 2), &no_use),
        Err(Error::Denied)
    );
    assert_eq!(g, before, "the child holds no new handle");
    // Observing it is not control.
    g.transfer(&t, 1, control, mine, ask(Rights::NONE, 2), &no_use)
        .unwrap();
    g.check(&t, &ch).unwrap();
}

/// Spec `cell-hierarchy`, "Revoked while passed on": A grants B a channel,
/// B passes it to C, and A's revocation takes both.
#[test]
fn revocation_takes_what_was_passed_on() {
    let mut t = root();
    child(&mut t, 2, 1, Egress::None, Egress::None); // B
    child(&mut t, 3, 1, Egress::None, Egress::None); // C
    let mut ch = Channels::new();
    let mut g = Grants::new();
    let a = Actor::Cell(1);
    let tool = ch.create(&t, a, &none).unwrap();
    let carry = ch.create(&t, a, &none).unwrap();
    let all = Rights::SEND | Rights::RECEIVE | Rights::CARRY;
    let [t0, _] = Channels::endpoints(tool);
    let [c0, c1] = Channels::endpoints(carry);
    let b = g
        .grant(&t, &ch, a, Object::Endpoint(t0), ask(all, 2))
        .unwrap();
    let bc = g
        .grant(&t, &ch, a, Object::Endpoint(c0), ask(all, 2))
        .unwrap();
    g.grant(&t, &ch, a, Object::Endpoint(c1), ask(all, 3))
        .unwrap();
    let c = g
        .transfer(
            &t,
            2,
            b,
            bc,
            ask(Rights::SEND | Rights::RECEIVE, 3),
            &no_use,
        )
        .unwrap();
    assert_eq!(g.get(c).unwrap().grantor(), 2, "B can revoke what it sent");
    // B lets its copy go: C's stays, still A's to revoke.
    g.unlink(2, b).unwrap();
    assert_eq!(g.get(c).unwrap().parent(), None);
    assert_eq!(g.get(c).unwrap().chain(), 1);
    let b2 = g
        .grant(&t, &ch, a, Object::Endpoint(t0), ask(all, 2))
        .unwrap();
    let c2 = g
        .transfer(&t, 2, b2, bc, ask(Rights::SEND, 3), &no_use)
        .unwrap();
    // Neither C nor a sibling may revoke A's grant; A takes the subtree.
    assert_eq!(
        g.revoke(&t, Actor::Cell(3), b2, &mut |_, _| {}),
        Err(Error::Denied)
    );
    let mut out = Vec::new();
    g.revoke(&t, Actor::Cell(1), b2, &mut collect(&mut out))
        .unwrap();
    out.sort_unstable();
    assert_eq!(out, [b2.min(c2), b2.max(c2)]);
    assert!(g.get(b2).is_none() && g.get(c2).is_none());
    assert!(g.get(c).is_some(), "the other grant stays");
    // Spec "Passed on, then closed": what B passed on and closed is still
    // A's to revoke.
    g.revoke(&t, Actor::Cell(1), c, &mut |_, _| {}).unwrap();
    assert!(g.get(c).is_none());
    g.check(&t, &ch).unwrap();
}

#[test]
fn revoke_derived_keeps_the_source() {
    let mut t = root();
    child(&mut t, 2, 1, Egress::None, Egress::None);
    child(&mut t, 3, 2, Egress::None, Egress::None);
    let ch = Channels::new();
    let mut g = Grants::new();
    let src = g
        .grant(
            &t,
            &ch,
            Actor::Host,
            Object::Cell(1),
            ask(Rights::OBSERVE, 2),
        )
        .unwrap();
    let d1 = g
        .derive(&t, 2, src, ask(Rights::OBSERVE, 3), &no_use)
        .unwrap();
    let d2 = g
        .derive(&t, 3, d1, ask(Rights::OBSERVE, 3), &no_use)
        .unwrap();
    assert_eq!(
        g.revoke_derived(3, src, &mut |_, _| {}),
        Err(Error::Denied),
        "not its holder"
    );
    let mut out = Vec::new();
    g.revoke_derived(2, src, &mut collect(&mut out)).unwrap();
    assert_eq!(out, [d2, d1], "post-order");
    assert!(g.get(src).is_some());
    // The host revokes what a cell granted, a cell not what the host did.
    assert_eq!(
        g.revoke(&t, Actor::Cell(2), src, &mut |_, _| {}),
        Err(Error::Denied)
    );
    g.revoke(&t, Actor::Host, src, &mut |_, _| {}).unwrap();
    assert_eq!(g.iter().count(), 0);
    g.check(&t, &ch).unwrap();
}

#[test]
fn bounds_of_handles_channels_and_chains() {
    let mut t = root();
    child(&mut t, 2, 1, Egress::None, Egress::None);
    let mut ch = Channels::new();
    let mut g = Grants::new();
    let src = g
        .grant(
            &t,
            &ch,
            Actor::Host,
            Object::Cell(1),
            ask(Rights::OBSERVE, 2),
        )
        .unwrap();
    for _ in 1..HANDLES_MAX {
        g.derive(&t, 2, src, ask(Rights::OBSERVE, 2), &no_use)
            .unwrap();
    }
    assert_eq!(
        g.derive(&t, 2, src, ask(Rights::OBSERVE, 2), &no_use),
        Err(Error::TooManyHandles)
    );
    // Endpoints count apart, up to their own bound.
    let c = ch.create(&t, Actor::Host, &none).unwrap();
    let e = g
        .grant(
            &t,
            &ch,
            Actor::Host,
            Object::Endpoint(2 * c),
            ask(Rights::SEND, 2),
        )
        .unwrap();
    for _ in 1..CHANNELS_MAX {
        g.derive(&t, 2, e, ask(Rights::SEND, 2), &no_use).unwrap();
    }
    assert_eq!(
        g.derive(&t, 2, e, ask(Rights::SEND, 2), &no_use),
        Err(Error::TooManyHandles)
    );
    // Chains: each grant derived from the last, in a cell with room.
    child(&mut t, 3, 1, Egress::None, Egress::None);
    let mut last = g
        .grant(
            &t,
            &ch,
            Actor::Host,
            Object::Cell(1),
            ask(Rights::OBSERVE, 3),
        )
        .unwrap();
    for _ in 1..CHAIN_MAX {
        last = g
            .derive(&t, 3, last, ask(Rights::OBSERVE, 3), &no_use)
            .unwrap();
    }
    assert_eq!(u32::from(g.get(last).unwrap().chain()), CHAIN_MAX);
    assert_eq!(
        g.derive(&t, 3, last, ask(Rights::OBSERVE, 3), &no_use),
        Err(Error::TooDeep)
    );
    g.check(&t, &ch).unwrap();
}

#[test]
fn cells_grant_only_within_their_subtree() {
    let mut t = root();
    child(&mut t, 2, 1, Egress::None, Egress::None);
    child(&mut t, 3, 1, Egress::None, Egress::None);
    let mut ch = Channels::new();
    let mut g = Grants::new();
    let c = ch.create(&t, Actor::Cell(2), &none).unwrap();
    let [e0, _] = Channels::endpoints(c);
    let (o, s) = (Rights::OBSERVE, Rights::SEND);
    let cases = [
        (Actor::Cell(2), Object::Cell(3), o, 2),      // a sibling
        (Actor::Cell(2), Object::Cell(1), o, 2),      // its parent
        (Actor::Cell(2), Object::Cell(2), o, 2),      // itself
        (Actor::Cell(2), Object::Endpoint(e0), s, 3), // to a sibling
        (Actor::Cell(3), Object::Endpoint(e0), s, 3), // another's channel
    ];
    for (by, object, rights, holder) in cases {
        assert_eq!(
            g.grant(&t, &ch, by, object, ask(rights, holder)),
            Err(Error::Denied),
            "{by:?} {object:?} to {holder}"
        );
    }
    g.grant(
        &t,
        &ch,
        Actor::Cell(2),
        Object::Endpoint(e0),
        ask(Rights::SEND, 2),
    )
    .unwrap();
    g.check(&t, &ch).unwrap();
}

/// Spec `cell-hierarchy`, "No proxy to the internet": a channel between a
/// child with egress `none` and one with `full` is refused until the
/// first one's ceiling is raised.
#[test]
fn no_proxy_past_the_ceiling() {
    let mut t = root();
    child(&mut t, 2, 1, Egress::None, Egress::None);
    child(&mut t, 3, 1, Egress::Full, Egress::Full);
    let mut ch = Channels::new();
    let mut g = Grants::new();
    let c = ch.create(&t, Actor::Cell(1), &none).unwrap();
    let [e0, e1] = Channels::endpoints(c);
    let both = Rights::SEND | Rights::RECEIVE;
    let p = Actor::Cell(1);
    let to_none = g
        .grant(&t, &ch, p, Object::Endpoint(e0), ask(both, 2))
        .unwrap();
    let before = g.clone();
    assert_eq!(
        g.grant(&t, &ch, p, Object::Endpoint(e1), ask(both, 3)),
        Err(Error::Denied)
    );
    assert_eq!(g, before);
    // One way only, from the cell without egress: still a way out.
    assert_eq!(
        g.grant(&t, &ch, p, Object::Endpoint(e1), ask(Rights::RECEIVE, 3)),
        Err(Error::Denied)
    );
    // From the cell with egress to the one without: nothing leaves.
    g.revoke(&t, p, to_none, &mut |_, _| {}).unwrap();
    g.grant(&t, &ch, p, Object::Endpoint(e0), ask(Rights::RECEIVE, 2))
        .unwrap();
    g.grant(&t, &ch, p, Object::Endpoint(e1), ask(Rights::SEND, 3))
        .unwrap();
    g.check(&t, &ch).unwrap();
    // Both ways once the host raises the first child's ceiling.
    t.set_egress(2, Egress::None, Egress::Full, Actor::Host)
        .unwrap();
    g.grant(&t, &ch, p, Object::Endpoint(e0), ask(both, 2))
        .unwrap();
    g.grant(&t, &ch, p, Object::Endpoint(e1), ask(both, 3))
        .unwrap();
    g.check(&t, &ch).unwrap();
}

/// A cell in the middle does not make a proxy: data from a cell without
/// egress cannot reach one with egress through a cell with a wider
/// ceiling, nor through an observe handle or a shared accelerator.
#[test]
fn no_proxy_through_a_middle_cell() {
    let mut t = root();
    child(&mut t, 2, 1, Egress::None, Egress::None);
    child(&mut t, 3, 1, Egress::None, Egress::Full);
    child(&mut t, 4, 1, Egress::Full, Egress::Full);
    let ch = Channels::new();
    let mut g = Grants::new();
    let h = Actor::Host;
    // 4 observes 3: 3's data flows to 4, within 3's ceiling.
    g.grant(&t, &ch, h, Object::Cell(3), ask(Rights::OBSERVE, 4))
        .unwrap();
    // 2 may not feed 3, nor be observed by it.
    assert_eq!(
        g.grant(&t, &ch, h, Object::Cell(3), ask(Rights::INPUT, 2)),
        Err(Error::Denied)
    );
    assert_eq!(
        g.grant(&t, &ch, h, Object::Cell(2), ask(Rights::OBSERVE, 3)),
        Err(Error::Denied)
    );
    // A message right alone is no flow: the channel it opens is checked.
    g.grant(&t, &ch, h, Object::Cell(3), ask(Rights::MESSAGE, 2))
        .unwrap();
    // 3 may feed 2: nothing leaves through 2.
    g.grant(&t, &ch, h, Object::Cell(2), ask(Rights::INPUT, 3))
        .unwrap();
    // Holders of one accelerator are joined both ways (design D24).
    let terms = AccelTerms {
        models: 1,
        budget: AccelBudget {
            sessions: 4,
            ..AccelBudget::ZERO
        },
    };
    g.register_accel(9, terms).unwrap();
    let one = Ask {
        accel: AccelTerms {
            models: 1,
            budget: AccelBudget {
                sessions: 1,
                ..AccelBudget::ZERO
            },
        },
        ..ask(Rights::INFER, 4)
    };
    g.grant(&t, &ch, h, Object::Accel(9), one.clone()).unwrap();
    assert_eq!(
        g.grant(&t, &ch, h, Object::Accel(9), Ask { holder: 2, ..one }),
        Err(Error::Denied)
    );
    g.check(&t, &ch).unwrap();
}

/// Ancestors are a cell's trusted base: a channel to its parent with
/// egress never counts against its ceiling.
#[test]
fn ancestors_do_not_count() {
    let mut t = root();
    child(&mut t, 2, 1, Egress::None, Egress::None);
    child(&mut t, 3, 2, Egress::None, Egress::None);
    let mut ch = Channels::new();
    let mut g = Grants::new();
    let c = ch.create(&t, Actor::Cell(1), &none).unwrap();
    let [e0, e1] = Channels::endpoints(c);
    let both = Rights::SEND | Rights::RECEIVE;
    g.grant(&t, &ch, Actor::Cell(1), Object::Endpoint(e0), ask(both, 1))
        .unwrap();
    g.grant(&t, &ch, Actor::Cell(1), Object::Endpoint(e1), ask(both, 3))
        .unwrap();
    g.check(&t, &ch).unwrap();
    // Moved out from under them, the grandchild would be a proxy.
    let fresh = &mut || 77;
    t.reparent(3, HOST, Actor::Host, &none, fresh).unwrap();
    assert_eq!(g.admits(&t, &[3]), Err(Error::Denied));
}

#[test]
fn a_cell_that_ends_takes_its_handles() {
    let mut t = root();
    child(&mut t, 2, 1, Egress::None, Egress::None);
    child(&mut t, 3, 1, Egress::None, Egress::None);
    let ch = Channels::new();
    let mut g = Grants::new();
    let on2 = g
        .grant(
            &t,
            &ch,
            Actor::Host,
            Object::Cell(2),
            ask(Rights::OBSERVE, 3),
        )
        .unwrap();
    let held = g
        .grant(
            &t,
            &ch,
            Actor::Host,
            Object::Cell(3),
            ask(Rights::OBSERVE, 2),
        )
        .unwrap();
    let passed = g
        .derive(&t, 3, on2, ask(Rights::OBSERVE, 3), &no_use)
        .unwrap();
    let kept = g
        .grant(
            &t,
            &ch,
            Actor::Host,
            Object::Cell(1),
            ask(Rights::OBSERVE, 2),
        )
        .unwrap();
    let from_kept = g
        .grant(
            &t,
            &ch,
            Actor::Host,
            Object::Cell(1),
            ask(Rights::OBSERVE, 3),
        )
        .unwrap();
    let mut out = Vec::new();
    t.end(2, &mut || 50).unwrap();
    g.cell_gone(2, &mut collect(&mut out));
    out.sort_unstable();
    let mut want = vec![on2, passed, held, kept];
    want.sort_unstable();
    assert_eq!(out, want);
    assert!(g.get(from_kept).is_some());
    g.check(&t, &ch).unwrap();
}

/// Copies inherit their source's cell handles, derived from them, but no
/// control handle on the source's descendants and no endpoint (design
/// D13).
#[test]
fn copies_inherit_cell_handles() {
    let mut t = root();
    child(&mut t, 2, 1, Egress::None, Egress::None);
    child(&mut t, 3, 1, Egress::None, Egress::None);
    child(&mut t, 4, 2, Egress::None, Egress::None);
    let mut ch = Channels::new();
    let mut g = Grants::new();
    let h = Actor::Host;
    let on3 = g
        .grant(
            &t,
            &ch,
            h,
            Object::Cell(3),
            ask(Rights::MESSAGE | Rights::CONTROL, 2),
        )
        .unwrap();
    g.grant(&t, &ch, h, Object::Cell(4), ask(Rights::CONTROL, 2))
        .unwrap();
    let c = ch.create(&t, h, &none).unwrap();
    g.grant(&t, &ch, h, Object::Endpoint(2 * c), ask(Rights::SEND, 2))
        .unwrap();
    t.add_copies(2, &[5, 6], &none, &mut |_, _, _| {}).unwrap();
    g.inherit(&t, 2, &[5, 6]).unwrap();
    for copy in [5, 6] {
        let held: Vec<&Grant> = g
            .iter()
            .map(|(_, x)| x)
            .filter(|x| x.holder() == copy)
            .collect();
        assert_eq!(held.len(), 1, "copy {copy}");
        assert_eq!(held[0].object(), Object::Cell(3));
        assert_eq!(held[0].parent(), Some(on3));
        assert_eq!(held[0].grantor(), HOST);
    }
    // Revoked together with the source's.
    g.revoke(&t, h, on3, &mut |_, _| {}).unwrap();
    assert!(g.iter().all(|(_, x)| x.holder() == 2));
    assert_eq!(g.inherit(&t, 2, &[5]), Ok(()));
    assert_eq!(g.inherit(&t, 2, &[99]), Err(Error::NotFound));
    g.check(&t, &ch).unwrap();
}

// Property tests.

#[derive(Debug, Clone)]
enum Op {
    /// A channel made by a cell or the host.
    Channel(Option<usize>),
    /// A channel the host makes between two cells, every right on each end.
    Pair(usize, usize),
    /// A grant by a cell or the host: an object (a cell, an endpoint, the
    /// accelerator), rights by bits, a holder.
    Grant(Option<usize>, u8, usize, u16, usize),
    /// A derivation: by the holder of a grant, rights by bits, a holder
    /// (itself or not).
    Derive(usize, u16, Option<usize>),
    /// A transfer of a grant over another to a cell.
    Transfer(usize, usize, usize, u16),
    Revoke(Option<usize>, usize),
    RevokeDerived(usize),
    Unlink(usize),
    End(usize),
    Copy(usize),
}

fn arb_op() -> impl Strategy<Value = Op> {
    let i = || 0usize..64;
    let rights = || 0u16..0x1000;
    prop_oneof![
        1 => prop::option::of(i()).prop_map(Op::Channel),
        2 => (i(), i()).prop_map(|(a, b)| Op::Pair(a, b)),
        4 => (prop::option::of(i()), 0u8..3, i(), rights(), i())
            .prop_map(|(by, k, o, r, h)| Op::Grant(by, k, o, r, h)),
        4 => (i(), rights(), prop::option::of(i())).prop_map(|(g, r, h)| Op::Derive(g, r, h)),
        2 => (i(), i(), i(), rights()).prop_map(|(g, o, h, r)| Op::Transfer(g, o, h, r)),
        1 => (prop::option::of(i()), i()).prop_map(|(by, g)| Op::Revoke(by, g)),
        1 => i().prop_map(Op::RevokeDerived),
        1 => i().prop_map(Op::Unlink),
        1 => i().prop_map(Op::End),
        1 => i().prop_map(Op::Copy),
    ]
}

/// A tree of two domains with policies of every kind: the first a root
/// with egress `full` and children with `none`, a model rule, `full` and
/// a raised ceiling; the second a root with the model rule.
fn world() -> Tree {
    let model = Egress::rules(&[rule(Proto::Tcp, [10, 0, 2, 100], 32, 8080, 8080)]).unwrap();
    let mut t = root();
    child(&mut t, 2, 1, Egress::None, Egress::None);
    child(&mut t, 3, 1, model.clone(), model.clone());
    child(&mut t, 4, 1, Egress::Full, Egress::Full);
    child(&mut t, 5, 1, Egress::None, Egress::Full);
    child(&mut t, 6, 2, Egress::None, Egress::None);
    child(&mut t, 7, 4, Egress::Full, Egress::Full);
    let s = Spec {
        egress: model.clone(),
        ..spec(limits(64 * MIB, 32))
    };
    t.add_root(10, s, 0).unwrap();
    child(&mut t, 11, 10, model.clone(), model);
    child(&mut t, 12, 10, Egress::None, Egress::None);
    t
}

/// The accelerator of the property tests.
const ACCEL: AccelTerms = AccelTerms {
    models: 0b111,
    budget: AccelBudget {
        mem: 1 << 30,
        kv_host: 1 << 28,
        time: 1000,
        sessions: 16,
    },
};

/// Terms from `bits`: some of the models and a share of `of`.
fn terms(bits: u16, of: &AccelTerms) -> AccelTerms {
    let part = |x: u64| x / 4 * u64::from(bits >> 10 & 3);
    AccelTerms {
        models: of.models & u64::from(bits),
        budget: AccelBudget {
            mem: part(of.budget.mem),
            kv_host: part(of.budget.kv_host),
            time: part(u64::from(of.budget.time)) as u32,
            sessions: part(u64::from(of.budget.sessions)) as u32,
        },
    }
}

/// What a handle's own sessions use in the property tests: a little, by
/// grant number.
fn use_of(id: u32) -> AccelBudget {
    AccelBudget {
        sessions: id % 2,
        ..AccelBudget::ZERO
    }
}

struct World {
    tree: Tree,
    channels: Channels,
    grants: Grants,
    next: u32,
    /// Channels made, numbered from 0 (none ends in these tests).
    made: u32,
}

/// Every right, by bit.
const ALL: [Rights; 10] = [
    Rights::OBSERVE,
    Rights::INPUT,
    Rights::MESSAGE,
    Rights::CONTROL,
    Rights::SEND,
    Rights::RECEIVE,
    Rights::CARRY,
    Rights::INFER,
    Rights::COMPUTE,
    Rights::STATS,
];

/// The rights of `object`'s kind among the bits of `bits`.
pub(super) fn rights_of(object: Object, bits: u16) -> Rights {
    ALL.iter()
        .enumerate()
        .filter(|(i, r)| bits >> i & 1 == 1 && object.rights().contains(**r))
        .fold(Rights::NONE, |a, (_, r)| a | *r)
}

/// Rights asked of grant `src`: among its own, or (bit 11 of `bits`) any of
/// its kind, which may be more than it holds.
fn asked(src: &Grant, bits: u16) -> Rights {
    let any = rights_of(src.object(), bits);
    if bits & 1 << 11 != 0 {
        return any;
    }
    ALL.iter()
        .filter(|r| any.contains(**r) && src.rights().contains(**r))
        .fold(Rights::NONE, |a, r| a | *r)
}

/// Runs `op`; returns the grants a revocation should have removed.
fn apply(w: &mut World, op: &Op) -> Result<Vec<u32>, Error> {
    let cells: Vec<u32> = w.tree.iter().map(|n| n.id()).collect();
    let ids: Vec<u32> = w.grants.iter().map(|(id, _)| id).collect();
    let cell = |i: usize| cells[i % cells.len()];
    let grant = |i: usize| ids.get(i % ids.len().max(1)).copied().unwrap_or(u32::MAX);
    let actor = |i: Option<usize>| i.map_or(Actor::Host, |i| Actor::Cell(cell(i)));
    let World {
        tree,
        channels,
        grants,
        next,
        made,
    } = w;
    match *op {
        Op::Channel(by) => {
            channels.create(tree, actor(by), &none)?;
            *made += 1;
            Ok(vec![])
        }
        Op::Pair(a, b) => {
            let c = channels.create(tree, Actor::Host, &none)?;
            *made += 1;
            let all = Rights::SEND | Rights::RECEIVE | Rights::CARRY;
            let [e0, e1] = Channels::endpoints(c);
            let ask = |h| Ask {
                rights: all,
                holder: cell(h),
                ..Ask::default()
            };
            grants.grant(tree, channels, Actor::Host, Object::Endpoint(e0), ask(a))?;
            // Two requests: the second may be refused on its own.
            let _ = grants.grant(tree, channels, Actor::Host, Object::Endpoint(e1), ask(b));
            Ok(vec![])
        }
        Op::Grant(by, kind, o, bits, h) => {
            let object = match kind {
                0 => Object::Cell(cell(o)),
                1 => Object::Endpoint(o as u32 % (2 * *made).max(1)),
                _ => Object::Accel(1),
            };
            let a = Ask {
                rights: rights_of(object, bits),
                holder: cell(h),
                label: Label::default(),
                accel: if kind == 2 {
                    terms(bits, &ACCEL)
                } else {
                    AccelTerms::default()
                },
            };
            grants
                .grant(tree, channels, actor(by), object, a)
                .map(|_| vec![])
        }
        Op::Derive(g, bits, h) => {
            let from = grant(g);
            let src = grants.get(from).cloned();
            let holder = src.as_ref().map_or(0, |s| s.holder());
            let a = Ask {
                rights: src.as_ref().map_or(Rights::NONE, |s| asked(s, bits)),
                holder: h.map_or(holder, cell),
                label: Label::default(),
                accel: src
                    .as_ref()
                    .map_or(AccelTerms::default(), |s| terms(bits, s.accel())),
            };
            let id = grants.derive(tree, holder, from, a, &use_of)?;
            let (src, new) = (src.unwrap(), grants.get(id).unwrap());
            assert!(
                src.rights().contains(new.rights()),
                "a request raised rights"
            );
            assert!(new.accel().within(src.accel()), "a request widened terms");
            Ok(vec![])
        }
        Op::Transfer(g, o, h, bits) => {
            // Mostly over an endpoint that carries, of a grant its holder
            // has, to a holder of the peer.
            let pick = |v: Vec<u32>, i: usize| v.get(i % v.len().max(1)).copied();
            let carry = |x: &Grant| {
                matches!(x.object(), Object::Endpoint(_)) && x.rights().contains(Rights::CARRY)
            };
            let over = pick(
                grants
                    .iter()
                    .filter(|(_, x)| carry(x))
                    .map(|(i, _)| i)
                    .collect(),
                o,
            )
            .unwrap_or_else(|| grant(o));
            let by = grants.get(over).map_or(0, |x| x.holder());
            let mine = grants
                .iter()
                .filter(|(_, x)| x.holder() == by)
                .map(|(i, _)| i);
            let from = pick(mine.collect(), g).unwrap_or_else(|| grant(g));
            let src = grants.get(from).cloned();
            let peer = match grants.get(over).map(|x| x.object()) {
                Some(Object::Endpoint(e)) => Object::Endpoint(e ^ 1),
                _ => Object::Cell(0),
            };
            let receivers = grants.iter().filter(|(_, x)| x.object() == peer);
            let receiver = pick(receivers.map(|(_, x)| x.holder()).collect(), h);
            let a = Ask {
                rights: src.as_ref().map_or(Rights::NONE, |s| asked(s, bits)),
                holder: receiver.unwrap_or_else(|| cell(h)),
                label: Label::default(),
                accel: src
                    .as_ref()
                    .map_or(AccelTerms::default(), |s| terms(bits, s.accel())),
            };
            let id = grants.transfer(tree, by, from, over, a, &use_of)?;
            let (src, new) = (src.unwrap(), grants.get(id).unwrap());
            assert!(
                src.rights().contains(new.rights()),
                "a transfer raised rights"
            );
            assert!(new.accel().within(src.accel()), "a transfer widened terms");
            Ok(vec![])
        }
        Op::Revoke(by, g) => {
            let id = grant(g);
            let sub = subtree(grants, id);
            let mut out = Vec::new();
            grants.revoke(tree, actor(by), id, &mut collect(&mut out))?;
            out.sort_unstable();
            assert_eq!(out, sub, "revocation removes the derivation subtree");
            Ok(sub)
        }
        Op::RevokeDerived(g) => {
            let id = grant(g);
            let by = grants.get(id).map_or(0, |x| x.holder());
            let mut sub = subtree(grants, id);
            sub.retain(|&x| x != id);
            let mut out = Vec::new();
            grants.revoke_derived(by, id, &mut collect(&mut out))?;
            out.sort_unstable();
            assert_eq!(out, sub, "revocation removes the derivation subtree");
            Ok(sub)
        }
        Op::Unlink(g) => {
            let id = grant(g);
            let by = grants.get(id).map_or(0, |x| x.holder());
            grants.unlink(by, id).map(|_| vec![])
        }
        Op::End(c) => {
            let id = cell(c);
            if id == 1 {
                return Err(Error::Denied);
            }
            let ended = tree.end(id, &mut || 999)?;
            for gone in ended.gone.iter().chain([&id]) {
                grants.cell_gone(*gone, &mut |_, _| {});
            }
            Ok(vec![])
        }
        Op::Copy(c) => {
            let source = cell(c);
            *next += 1;
            let copy = *next;
            tree.add_copies(source, &[copy], &none, &mut |_, _, _| {})?;
            let r = grants.inherit(tree, source, &[copy]);
            if r.is_err() {
                tree.end(copy, &mut || 999).unwrap();
            }
            r.map(|_| vec![])
        }
    }
}

/// Grant `id` and every grant derived from it, by their parents.
fn subtree(g: &Grants, id: u32) -> Vec<u32> {
    let mut out: Vec<u32> = g.iter().map(|(x, _)| x).filter(|&x| x == id).collect();
    loop {
        let more: Vec<u32> = g
            .iter()
            .filter(|(x, y)| !out.contains(x) && y.parent().is_some_and(|p| out.contains(&p)))
            .map(|(x, _)| x)
            .collect();
        if more.is_empty() {
            out.sort_unstable();
            return out;
        }
        out.extend(more);
    }
}

fn world_state() -> World {
    let mut grants = Grants::new();
    grants.register_accel(1, ACCEL).unwrap();
    World {
        tree: world(),
        channels: Channels::new(),
        grants,
        next: 100,
        made: 0,
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 200, ..ProptestConfig::default() })]

    /// After any sequence of grants, derivations, transfers, revocations,
    /// ends and copies: no request raised rights, every revocation removed
    /// exactly the derivation subtree, control never flowed up, every
    /// exposure is within its ceiling, and every refusal changed nothing.
    #[test]
    fn grants_hold_under_any_operations(ops in prop::collection::vec(arb_op(), 1..60)) {
        let mut w = world_state();
        for op in &ops {
            let before = (w.grants.clone(), w.tree.clone());
            if apply(&mut w, op).is_err() {
                prop_assert_eq!(&w.grants, &before.0, "{:?} failed but changed the grants", op);
            }
            prop_assert_eq!(w.grants.check(&w.tree, &w.channels), Ok(()), "after {:?}", op);
            prop_assert_eq!(w.tree.check(), Ok(()));
        }
    }

    /// Every operation with any one of its allocations made to fail
    /// reports it and leaves the grants unchanged.
    #[test]
    fn grants_failed_allocations_change_nothing(ops in prop::collection::vec(arb_op(), 1..40)) {
        let mut w = world_state();
        for op in &ops {
            for k in 0.. {
                let before = (w.grants.clone(), w.tree.clone(), w.channels.clone(), w.next, w.made);
                FAIL_IN.with(|f| f.set(Some(k)));
                let r = apply(&mut w, op);
                FAIL_IN.with(|f| f.set(None));
                let failed = FAILED.with(|f| f.replace(false));
                if failed && r.is_err() {
                    prop_assert_eq!(r, Err(Error::NoMemory));
                    prop_assert_eq!(&w.grants, &before.0, "{:?} changed the grants", op);
                    (w.tree, w.channels, w.next, w.made) = (before.1, before.2, before.3, before.4);
                    continue;
                }
                prop_assert_eq!(w.grants.check(&w.tree, &w.channels), Ok(()));
                break;
            }
        }
    }
}

/// A handle sent to one holder of an endpoint and taken by another holder
/// of it becomes the taker's, after the checks every grant passes; a cell
/// that holds no receiving grant on the endpoint cannot take it.
#[test]
fn rehome_moves_a_received_handle() {
    let mut t = root();
    child(&mut t, 2, 1, Egress::None, Egress::None); // B
    child(&mut t, 3, 1, Egress::None, Egress::None); // C
    child(&mut t, 4, 1, Egress::None, Egress::None); // D
    let mut ch = Channels::new();
    let mut g = Grants::new();
    let a = Actor::Cell(1);
    let carry = ch.create(&t, a, &none).unwrap();
    let tool = ch.create(&t, a, &none).unwrap();
    let all = Rights::SEND | Rights::RECEIVE | Rights::CARRY;
    let [c0, c1] = Channels::endpoints(carry);
    let [t0, _] = Channels::endpoints(tool);
    let mine = g
        .grant(&t, &ch, a, Object::Endpoint(c0), ask(all, 1))
        .unwrap();
    g.grant(&t, &ch, a, Object::Endpoint(c1), ask(all, 2))
        .unwrap();
    g.grant(&t, &ch, a, Object::Endpoint(c1), ask(Rights::RECEIVE, 3))
        .unwrap();
    g.grant(&t, &ch, a, Object::Endpoint(c1), ask(Rights::SEND, 4))
        .unwrap();
    let src = g
        .grant(&t, &ch, a, Object::Endpoint(t0), ask(all, 1))
        .unwrap();
    let sent = g
        .transfer(&t, 1, src, mine, ask(Rights::SEND, 2), &no_use)
        .unwrap();
    let before = g.clone();
    assert_eq!(
        g.rehome(&t, sent, 4, Object::Endpoint(c1)),
        Err(Error::Denied),
        "D may only send on the endpoint"
    );
    assert_eq!(g, before, "a refused move changes nothing");
    g.rehome(&t, sent, 3, Object::Endpoint(c1)).unwrap();
    let moved = g.get(sent).unwrap();
    assert_eq!((moved.holder(), moved.grantor()), (3, 1));
    assert_eq!(moved.parent(), Some(src), "still revocable by the sender");
    assert_eq!(g.rehome(&t, sent, 3, Object::Endpoint(c1)), Ok(()));
    g.check(&t, &ch).unwrap();
}
