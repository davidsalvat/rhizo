//! The CPython agent's output as the host that runs it reads it (spec
//! `workloads`, change `fix-agent-turn-race`): a turn ends with a line
//! starting `turn `, and its decision point is a line ending in `fork`.
//! The host reads the output in whatever pieces the cell's slices leave,
//! so a turn's line and what follows it can come in one piece; [`Watch`]
//! finds every complete line however the bytes are split.

/// Lines found in the bytes read by one [`Watch::read`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Seen {
    /// Turns that ended: complete lines starting `turn `.
    pub turns: usize,
    /// Whether the decision point came: a complete line ending in `fork`.
    pub fork: bool,
}

const TURN: &[u8; 5] = b"turn ";
const FORK: &[u8; 4] = b"fork";

/// Reads the agent's output a piece at a time, keeping the start and end
/// of the line not yet complete.
#[derive(Debug, Default)]
pub struct Watch {
    /// Bytes of the current line so far.
    len: usize,
    /// Its first bytes.
    head: [u8; TURN.len()],
    /// Its last bytes, the latest at the end.
    tail: [u8; FORK.len()],
}

impl Watch {
    /// Reads the next `bytes` of the output: the lines they complete.
    pub fn read(&mut self, bytes: &[u8]) -> Seen {
        let mut seen = Seen::default();
        for &b in bytes {
            if b == b'\n' {
                seen.turns += usize::from(self.len >= TURN.len() && self.head == *TURN);
                seen.fork |= self.len >= FORK.len() && self.tail == *FORK;
                *self = Self::default();
                continue;
            }
            if let Some(h) = self.head.get_mut(self.len) {
                *h = b;
            }
            self.tail.copy_within(1.., 0);
            self.tail[FORK.len() - 1] = b;
            self.len = self.len.saturating_add(1);
        }
        seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// The agent's output in `timed` mode up to its decision point, with
    /// `turns` turns.
    fn output(turns: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for t in 1..=turns {
            out.extend_from_slice(
                format!("turn {t} call_37dd1708 n=804 seed=565 value=209594 conn_requests={t}\n")
                    .as_bytes(),
            );
        }
        out.extend_from_slice(b"agent turn_ns=1,2,3\nagent model_ns=1,2,3\nfork\n");
        out
    }

    #[test]
    fn a_turn_and_the_fork_in_one_piece_are_both_seen() {
        // The last turn's pause passed while the host did not look
        // (GitHub Actions run 44941595429): the turn's line and the
        // decision point arrive together.
        let mut w = Watch::default();
        assert_eq!(
            w.read(b"turn 3 call_4801f3f6 n=898 conn_requests=3\nfork\n"),
            Seen {
                turns: 1,
                fork: true
            }
        );
    }

    #[test]
    fn incomplete_and_other_lines_are_not_turns() {
        let mut w = Watch::default();
        assert_eq!(w.read(b"turn 1 n=804"), Seen::default());
        assert_eq!(w.read(b"\n").turns, 1);
        for line in [
            &b"turn\n"[..],
            b"turn1\n",
            b" turn 2\n",
            b"agent turn_ns=1\n",
            b"\n",
        ] {
            assert_eq!(w.read(line), Seen::default(), "{line:?}");
        }
        assert!(!w.read(b"for").fork);
        assert!(!w.read(b"k").fork, "not complete");
        assert!(w.read(b"\n").fork);
        assert!(w.read(b"[agent] decision point: asking for a fork\n").fork);
        assert!(!w.read(b"fork 1\n").fork);
    }

    proptest! {
        /// However the output is split, every turn is seen once, in order,
        /// and the decision point once, after the last turn.
        #[test]
        fn every_turn_is_seen_however_the_bytes_arrive(
            turns in 0usize..12,
            cuts in proptest::collection::vec(0usize..2000, 0..40),
        ) {
            let out = output(turns);
            let mut cuts: Vec<usize> = cuts.into_iter().map(|c| c % (out.len() + 1)).collect();
            cuts.push(out.len());
            cuts.sort_unstable();
            let mut w = Watch::default();
            let (mut seen_turns, mut forks, mut from) = (0, 0, 0);
            for cut in cuts {
                let seen = w.read(&out[from..cut]);
                prop_assert!(forks == 0 || seen.turns == 0, "a turn after the fork");
                seen_turns += seen.turns;
                forks += usize::from(seen.fork);
                from = cut;
            }
            prop_assert_eq!(seen_turns, turns);
            prop_assert_eq!(forks, 1);
        }
    }
}
