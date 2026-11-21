use super::*;
use proptest::prelude::*;

#[test]
fn time_goes_to_the_innermost_part() {
    let mut a = Account::new();
    a.start(100);
    a.enter(Part::Syscall, 110); // 10 to other
    a.enter(Part::Copy, 130); // 20 to syscall
    assert_eq!(a.leave(135), 5); // 5 to copy
    assert_eq!(a.leave(150), 40); // 15 more to syscall
    a.charge(160); // 10 more to other
    assert_eq!(a.time(Part::Other), 20);
    assert_eq!(a.time(Part::Syscall), 35);
    assert_eq!(a.time(Part::Copy), 5);
    assert_eq!(a.inclusive(Part::Syscall), 40);
    assert_eq!(a.count(Part::Syscall), 1);
    assert_eq!(a.total(), 60);
}

#[test]
fn nested_scopes_of_one_part_count_once_inclusive() {
    let mut a = Account::new();
    a.enter(Part::Frames, 0);
    a.enter(Part::Frames, 2);
    a.leave(3);
    a.leave(10);
    assert_eq!(a.inclusive(Part::Frames), 10);
    assert_eq!(a.time(Part::Frames), 10);
    assert_eq!(a.count(Part::Frames), 2);
}

#[test]
fn scopes_beyond_the_depth_go_to_their_parent() {
    let mut a = Account::new();
    for i in 0..DEPTH + 2 {
        a.enter(Part::Copy, i as u64);
    }
    for i in 0..DEPTH + 2 {
        a.leave(100 + i as u64);
    }
    assert_eq!(a.total(), 100 + DEPTH as u64 + 1);
    assert_eq!(a.count(Part::Copy), DEPTH as u64);
    a.enter(Part::Cell, 200);
    a.leave(210);
    assert_eq!(a.time(Part::Cell), 10, "balanced again");
}

#[test]
fn leaving_a_scope_opened_before_the_start_is_harmless() {
    let mut a = Account::new();
    a.start(5);
    assert_eq!(a.leave(9), 0);
    a.charge(10);
    assert_eq!(a.time(Part::Other), 5);
}

#[test]
fn accounts_merge() {
    let (mut a, mut b) = (Account::new(), Account::new());
    a.enter(Part::Idle, 0);
    a.leave(7);
    b.enter(Part::Idle, 0);
    b.leave(3);
    a.merge(&b);
    assert_eq!(a.time(Part::Idle), 10);
    assert_eq!(a.count(Part::Idle), 2);
}

#[test]
fn top_calls_by_time() {
    let mut c = Calls::new();
    c.add(1, 5);
    c.add(2, 50);
    c.add(3, 20);
    c.add(3, 20);
    c.add(9999, 1); // shares the last slot
    let (top, n) = c.top::<2>();
    assert_eq!(n, 2);
    assert_eq!(top, [(2, 1, 50), (3, 2, 40)]);
    let (all, n) = c.top::<8>();
    assert_eq!(n, 4);
    assert_eq!(all[3], (CALL_SLOTS as u64 - 1, 1, 1));
    let mut d = Calls::new();
    d.add(1, 100);
    c.merge(&d);
    assert_eq!(c.top::<1>().0, [(1, 2, 105)]);
}

#[test]
fn breakdown_lines() {
    let mut a = Account::new();
    a.start(0);
    for i in 0..4 {
        a.enter(Part::Cell, i * 100);
        a.leave(i * 100 + 50);
    }
    a.enter(Part::Syscall, 400);
    a.leave(430);
    let b = Breakdown {
        workload: "w",
        account: &a,
        per_trap: 10,
    };
    let mut out = String::new();
    b.write(&mut out, |t| t * 2).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines[0].starts_with(
            "rhizo-breakdown w total_ns=860 user_ns=320 entry_exit_ns=80 syscall_ns=60 fault_ns=0"
        ),
        "{out}"
    );
    assert!(lines[0].ends_with("other_ns=400"), "{out}");
    assert!(
        lines[1].starts_with("rhizo-breakdown-counts w traps=4 syscall=1 fault=0"),
        "{out}"
    );
}

#[test]
fn entry_exit_never_exceeds_cell_time() {
    let mut a = Account::new();
    a.enter(Part::Cell, 0);
    a.leave(5);
    let b = Breakdown {
        workload: "w",
        account: &a,
        per_trap: 1000,
    };
    assert_eq!(b.entry_exit(), 5);
}

#[derive(Debug, Clone)]
enum Step {
    Enter(usize),
    Leave,
    Charge,
}

fn steps() -> impl Strategy<Value = Vec<(Step, u64)>> {
    prop::collection::vec(
        (
            prop_oneof![
                (0..PARTS).prop_map(Step::Enter),
                Just(Step::Leave),
                Just(Step::Charge)
            ],
            0u64..1000,
        ),
        0..200,
    )
}

proptest! {
    /// Whatever the scopes, the parts add up to the time accounted and no
    /// part's exclusive time exceeds its inclusive time once closed.
    #[test]
    fn parts_add_up_to_the_time_accounted(steps in steps()) {
        let mut a = Account::new();
        a.start(0);
        let mut now = 0;
        for (step, dt) in steps {
            now += dt;
            match step {
                Step::Enter(p) => a.enter(Part::ALL[p], now),
                Step::Leave => { a.leave(now); }
                Step::Charge => a.charge(now),
            }
        }
        while a.depth > 0 || a.beyond > 0 {
            a.leave(now);
        }
        a.charge(now);
        prop_assert_eq!(a.total(), now);
        for p in Part::ALL {
            if p != Part::Other {
                prop_assert!(a.time(p) <= a.inclusive(p));
            }
        }
    }
}
