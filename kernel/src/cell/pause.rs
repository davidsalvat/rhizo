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
    /// unseen. Once the line is out, a step runs this cell whenever it can
    /// run, before any other (design D1 of `fix-node-pause`): its pause
    /// may start while another of its threads can still run (Node.js's
    /// 50 ms timer, armed while a V8 worker it woke is runnable), and the
    /// pause must not end while that thread waits behind other cells'
    /// slices. An error says why the cell cannot pause there (it ended, or
    /// printed past the line) with what it printed, so a caller reports it
    /// as it is.
    pub fn run_to_pause(&mut self, id: u32, line: &[u8], pause: Pause) -> Result<(), String> {
        self.with_live(id, |live| live.hold = true)
            .ok_or_else(|| alloc::format!("cell {id} is not live"))?;
        let mut seen = Line::new(line);
        let mut read = 0;
        // The line is out and nothing after it: the cell heads to its pause.
        let mut settling = false;
        loop {
            if !(settling && self.step_held(id)) {
                self.step();
            }
            let after = {
                let Some(out) = self.output(id) else {
                    return Err(match self.finished(id) {
                        Some(f) => alloc::format!(
                            "cell {id} ended before it paused ({:?}), having printed {:?}",
                            f.ending,
                            String::from_utf8_lossy(&f.output)
                        ),
                        None => alloc::format!("cell {id} ended before it paused"),
                    });
                };
                seen.read(&out[read..]);
                read = out.len();
                seen.after()
            };
            match after {
                Some(0) if pause == Pause::Slice || self.waits(id) => break,
                Some(0) => settling = true,
                None => {}
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

    /// Runs one slice of live cell `id` on this CPU alone, as
    /// [`step`](Self::step) would once its turn came, if it is runnable
    /// now; returns whether it was.
    fn step_held(&mut self, id: u32) -> bool {
        let now = now_ns();
        let taken = self.locked(|s| {
            let mut live = s.sched.take(id)?;
            if s.nudged.remove(&id) {
                live.cell.threads.wake_io();
            }
            live.ready.ran(now);
            live.ready = Ready::default();
            s.running += 1;
            s.on_cpu[0] = id;
            s.idle &= !1;
            Some(live)
        });
        let Some(live) = taken else {
            return false;
        };
        run_slice(&self.shared, 0, live, now, false);
        self.collect();
        true
    }
}
