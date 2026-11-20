//! Samples of where cell code was when a CPU's sampling interrupt came
//! (spec `profiling`, sampling): a buffer per CPU, its room reserved before
//! sampling starts and filled as the interrupts arrive, then folded into
//! counts per cell and address before they are printed.

use alloc::vec::Vec;
use core::fmt::{self, Write};

/// Where a cell was: its identifier and the address of the instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Sample {
    pub cell: u32,
    pub pc: u64,
}

/// Up to a fixed number of samples; later ones are counted as lost, so
/// recording one never allocates.
#[derive(Debug, Default)]
pub struct Samples {
    buf: Vec<Sample>,
    lost: u64,
}

impl Samples {
    /// Room for `capacity` samples.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
            lost: 0,
        }
    }

    /// Keeps `sample`, or counts it as lost when the buffer is full.
    pub fn push(&mut self, sample: Sample) {
        if self.buf.len() < self.buf.capacity() {
            self.buf.push(sample);
        } else {
            self.lost += 1;
        }
    }

    /// Samples dropped because the buffer was full.
    pub fn lost(&self) -> u64 {
        self.lost
    }

    /// Moves `other`'s samples here, as far as there is room.
    pub fn append(&mut self, other: &Samples) {
        for &s in &other.buf {
            self.push(s);
        }
        self.lost += other.lost;
    }

    /// The samples kept, sorted by cell and address, so equal ones are
    /// next to each other ([`fold`]).
    pub fn sorted(&mut self) -> &[Sample] {
        self.buf.sort_unstable();
        &self.buf
    }
}

/// Equal samples of a sorted slice, counted: `(sample, count)` in order.
pub fn fold(sorted: &[Sample]) -> impl Iterator<Item = (Sample, u64)> + '_ {
    let mut rest = sorted;
    core::iter::from_fn(move || {
        let first = *rest.first()?;
        let n = rest.iter().take_while(|s| **s == first).count();
        rest = &rest[n..];
        Some((first, n as u64))
    })
}

/// Writes the folded samples of `workload` as the lines `xtask` reads:
/// `rhizo-sample <workload> <cell> <pc in hex> <count>`.
pub fn write(w: &mut impl Write, workload: &str, sorted: &[Sample]) -> fmt::Result {
    for (s, count) in fold(sorted) {
        writeln!(w, "rhizo-sample {workload} {} {:x} {count}", s.cell, s.pc)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(cell: u32, pc: u64) -> Sample {
        Sample { cell, pc }
    }

    #[test]
    fn samples_fold_into_counts_per_cell_and_address() {
        let mut b = Samples::with_capacity(8);
        for x in [s(2, 0x10), s(1, 0x20), s(2, 0x10), s(1, 0x10), s(2, 0x10)] {
            b.push(x);
        }
        let folded: Vec<_> = fold(b.sorted()).collect();
        assert_eq!(folded, [(s(1, 0x10), 1), (s(1, 0x20), 1), (s(2, 0x10), 3)]);
        let mut out = String::new();
        write(&mut out, "w", b.sorted()).unwrap();
        assert_eq!(
            out,
            "rhizo-sample w 1 10 1\nrhizo-sample w 1 20 1\nrhizo-sample w 2 10 3\n"
        );
    }

    #[test]
    fn a_full_buffer_counts_what_it_loses() {
        let mut b = Samples::with_capacity(2);
        for pc in 0..5 {
            b.push(s(1, pc));
        }
        assert_eq!(b.sorted().len(), 2);
        assert_eq!(b.lost(), 3);
        let mut all = Samples::with_capacity(3);
        all.append(&b);
        all.append(&b);
        assert_eq!((all.sorted().len(), all.lost()), (3, 7));
        assert_eq!(fold(&[]).count(), 0);
    }
}
