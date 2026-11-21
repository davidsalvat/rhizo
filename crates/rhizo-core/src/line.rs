//! One whole line in a cell's output as the host reads it (change
//! `fix-warm-wait-race`): a host waits for a template's `warm` line before
//! freezing it, and reads the output only between the cell's slices, in
//! whatever pieces they leave. [`Line`] finds the line however the bytes
//! are split, and counts what came after it.

/// Reads output a piece at a time for the first complete line equal to a
/// given one, keeping only how much of the current line matches so far.
#[derive(Debug, Clone)]
pub struct Line<'a> {
    /// The line looked for, without its newline.
    want: &'a [u8],
    /// Bytes of the current line that match `want` so far; `None` once it
    /// cannot match.
    matched: Option<usize>,
    /// Bytes read after the line, once it was found.
    after: Option<usize>,
}

impl<'a> Line<'a> {
    /// Looks for the line `want` (without its newline) from the start of
    /// the output.
    pub fn new(want: &'a [u8]) -> Self {
        Self {
            want,
            matched: Some(0),
            after: None,
        }
    }

    /// Reads the next `bytes` of the output.
    pub fn read(&mut self, bytes: &[u8]) {
        for (i, &b) in bytes.iter().enumerate() {
            if let Some(after) = self.after.as_mut() {
                *after += bytes.len() - i;
                return;
            }
            self.matched = if b == b'\n' {
                if self.matched == Some(self.want.len()) {
                    self.after = Some(0);
                }
                Some(0)
            } else {
                self.matched
                    .filter(|&n| self.want.get(n) == Some(&b))
                    .map(|n| n + 1)
            };
        }
    }

    /// Bytes read after the line, `None` while it has not come whole.
    pub fn after(&self) -> Option<usize> {
        self.after
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn the_pause_passed_unseen() {
        // A template's pause after `warm` ended inside the slice that
        // printed it: the line and the answer arrive in one piece.
        let mut line = Line::new(b"warm");
        line.read(b"warm\ncpython ok\n");
        assert_eq!(line.after(), Some(11));
    }

    #[test]
    fn only_a_whole_equal_line_counts() {
        let mut line = Line::new(b"warm");
        for piece in [
            &b"warm"[..],
            b"ing\n",
            b"a warm\n",
            b"war\n",
            b"\n",
            b"warmer\n",
            b"wa",
            b"rm",
        ] {
            line.read(piece);
            assert_eq!(line.after(), None, "{piece:?}");
        }
        line.read(b"\n");
        assert_eq!(line.after(), Some(0));
        line.read(b"x");
        line.read(b"warm\n");
        assert_eq!(line.after(), Some(6), "only the first one");
    }

    proptest! {
        /// However the output is split, the line is found once all of it
        /// was read, never before, and what follows it is counted.
        #[test]
        fn the_line_is_found_however_the_bytes_arrive(
            before in proptest::collection::vec("[a-z ]{0,8}", 0..6),
            rest in "[a-z\n]{0,40}",
            cuts in proptest::collection::vec(0usize..200, 0..20),
        ) {
            let mut out: Vec<u8> = Vec::new();
            for l in &before {
                if l != "warm" {
                    out.extend_from_slice(l.as_bytes());
                    out.push(b'\n');
                }
            }
            let end = out.len() + b"warm\n".len();
            out.extend_from_slice(b"warm\n");
            out.extend_from_slice(rest.as_bytes());
            let mut cuts: Vec<usize> = cuts.into_iter().map(|c| c % (out.len() + 1)).collect();
            cuts.push(out.len());
            cuts.sort_unstable();
            let mut line = Line::new(b"warm");
            let mut from = 0;
            for cut in cuts {
                line.read(&out[from..cut]);
                let want = (cut >= end).then(|| cut - end);
                prop_assert_eq!(line.after(), want);
                from = cut;
            }
        }
    }
}
