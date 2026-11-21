//! `--part K/N`: one share of the test steps, so several machines run a
//! full `test` or `ci` between them (change `faster-ci-2`).
//!
//! The steps are dealt by their estimated cost, most expensive first, each
//! to the part with the least so far. The deal depends only on the list of
//! steps and `N`, never on what a run selects, so the parts of one `N` hold
//! every step exactly once; a step missing from [`COSTS`] costs 1 and is
//! still dealt.

/// Estimated host CPU-seconds of each step (VM time multiplied by the host
/// CPUs it keeps busy) on GitHub's 4-CPU ARM64 runner, from CI run
/// 44932285327 (design `faster-ci-2`).
const COSTS: &[(&str, u32)] = &[
    ("ktest-smp", 168),
    ("host", 134),
    ("ktest", 92),
    ("control", 69),
    ("ktest-node", 52),
    ("ktest-packages", 38),
    ("differential", 14),
    ("ktest-hang", 5),
];

fn cost(step: &str) -> u32 {
    COSTS
        .iter()
        .find(|(s, _)| *s == step)
        .map_or(1, |&(_, c)| c)
}

/// Parses `K/N` (1 <= K <= N).
pub fn parse(value: &str) -> Result<(usize, usize), String> {
    let bad = || format!("--part needs K/N with 1 <= K <= N, not `{value}`");
    let (k, n) = value.split_once('/').ok_or_else(bad)?;
    let (k, n) = (
        k.parse::<usize>().map_err(|_| bad())?,
        n.parse::<usize>().map_err(|_| bad())?,
    );
    if k == 0 || k > n {
        return Err(bad());
    }
    Ok((k, n))
}

/// The part (1 to `n`) that runs `step`, among `steps`.
pub fn part_of(step: &str, steps: &[&str], n: usize) -> usize {
    let mut order: Vec<&str> = steps.to_vec();
    // Most expensive first; ties keep the list's order.
    order.sort_by_key(|s| std::cmp::Reverse(cost(s)));
    let mut load = vec![0u32; n.max(1)];
    for s in order {
        let part = (0..load.len()).min_by_key(|&p| load[p]).unwrap_or(0);
        if s == step {
            return part + 1;
        }
        load[part] += cost(s);
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEPS: &[&str] = &[
        "host",
        "differential",
        "boot",
        "ktest",
        "ktest-smp",
        "ktest-node",
        "ktest-packages",
        "control",
        "ktest-should-panic",
        "ktest-hang",
        "ktest-new-step",
    ];

    #[test]
    fn every_step_runs_in_exactly_one_part() {
        for n in 1..=5 {
            for step in STEPS {
                let part = part_of(step, STEPS, n);
                assert!((1..=n).contains(&part), "{step} in part {part} of {n}");
            }
        }
    }

    #[test]
    fn two_parts_share_the_cost() {
        let load = |k| -> u32 {
            STEPS
                .iter()
                .filter(|s| part_of(s, STEPS, 2) == k)
                .map(|s| cost(s))
                .sum()
        };
        let (a, b) = (load(1), load(2));
        assert!(a.abs_diff(b) <= 168 / 4, "{a} against {b}");
        // The two longest steps go to different machines.
        assert_ne!(part_of("ktest-smp", STEPS, 2), part_of("host", STEPS, 2));
    }

    #[test]
    fn part_values_are_checked() {
        assert_eq!(parse("1/2"), Ok((1, 2)));
        assert_eq!(parse("3/3"), Ok((3, 3)));
        for bad in ["0/2", "3/2", "1", "a/2", "1/", "/2"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
