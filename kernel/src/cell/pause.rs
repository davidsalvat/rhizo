//! Running a cell to the pause after a line it prints, where a host
//! freezes a template (change `fix-warm-wait-race`).

use super::*;
use alloc::string::String;
use rhizo_core::line::Line;

/// Where a cell pauses after its line ([`Host::run_to_pause`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pause {
    /// It waits: none of its threads can run (a sleep, a timer).
    Waits,
    /// The slice that printed the line ended: at the cell's next yield
    /// or block, or earlier if it was preempted.
    Slice,
}

impl Host {
    /// Steps until live cell `id` has printed the whole line `line`
    /// (without its newline) and nothing after it, and has reached
    /// `pause`; its output must be captured. Meanwhile a call of the cell
    /// that blocks ends its slice (design D1 of `fix-warm-wait-race`): a
    /// pause already over when its call blocks (a 1 ms sleep on a loaded
    /// host) still ends the slice, instead of letting the cell run past it
    /// unseen. An error says why the cell cannot pause there: it ended, or
    /// printed past the line.
    pub fn run_to_pause(&mut self, id: u32, line: &[u8], pause: Pause) -> Result<(), String> {
        self.with_live(id, |live| live.hold = true)
            .ok_or_else(|| alloc::format!("cell {id} is not live"))?;
        let mut seen = Line::new(line);
        let mut read = 0;
        loop {
            self.step();
            let after = {
                let Some(out) = self.output(id) else {
                    return Err(alloc::format!("cell {id} ended before it paused"));
                };
                seen.read(&out[read..]);
                read = out.len();
                seen.after()
            };
            match after {
                Some(0) if pause == Pause::Slice || self.waits(id) => break,
                Some(0) | None => {}
                Some(_) => {
                    let out = self.output(id).unwrap_or_default();
                    return Err(alloc::format!(
                        "cell {id} printed past its pause: {:?}",
                        String::from_utf8_lossy(out)
                    ));
                }
            }
        }
        self.with_live(id, |live| live.hold = false);
        Ok(())
    }
}
