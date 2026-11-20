//! The control kernel (spec `control-plane`, change `add-control-plane`):
//! after booting it serves the host's requests on the serial line, the
//! console's own, instead of running tests or benchmarks. It only does what
//! it is asked: which programs, with which files and when to suspend, fork
//! or kill them is the host's business (`rhizo`, design D1).
//!
//! A cell keeps its identifier for its whole life here: the kernel gives a
//! resumed snapshot a new one, which this table hides.

use crate::arch::console::control as link;
use crate::arch::timer::{self, Counter};
use crate::cell::{self, Ending, Host, Resume, now_ns};
use crate::memory;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use rhizo_control::{
    Body, CellInfo, Decoder, Error, Item, Message, Request, Run, State, Stats, VERSION,
};
use rhizo_hal::CycleCounter;
use rhizo_mm::FRAME_SIZE;
use rhizo_snapshot::Snapshot;

/// How often the boot CPU looks at the serial line while it has nothing to
/// run: often right after a request, less when the host is quiet (design
/// D4).
const LOOK_FAST_NS: u64 = 50_000;
const LOOK_SLOW_NS: u64 = 2_000_000;
/// How long after a request the kernel keeps looking often.
const FAST_FOR_NS: u64 = 200_000_000;
/// Output of watched cells goes to the host at least this often.
const OUTPUT_EVERY_NS: u64 = 10_000_000;
/// The rest of a frame must come within this long of its last byte.
const FRAME_GAP_NS: u64 = 1_000_000_000;
/// Most output kept for a cell nobody watches (as a cell's own capture).
const KEPT_OUTPUT: usize = 1 << 20;
/// Most cells one fork makes.
const MAX_FORK: u32 = 1024;

/// Where a cell is.
enum Place {
    /// Running as this kernel cell.
    Live(u32),
    Suspended(Snapshot),
    Ended(State),
}

/// A cell the host started, and what it printed that the host has not had.
struct Entry {
    id: u32,
    image: Vec<u8>,
    place: Place,
    /// The kernel cell it was last (to match it when it ends).
    cell: u32,
    out: Vec<u8>,
    /// Requests streaming its output.
    watchers: Vec<u32>,
}

struct Server {
    host: Host,
    cells: Vec<Entry>,
    decoder: Decoder,
    /// Frames and refusals read, not yet handled.
    items: Vec<Item>,
}

/// A request's outcome.
type Outcome = Result<Body, (Error, String)>;

fn refuse<T>(error: Error) -> Result<T, (Error, String)> {
    Err((error, String::new()))
}

fn send(id: u32, message: &Message) {
    link::write(&message.encode(id));
}

/// Serves requests until the host asks for the power off.
pub fn serve() {
    let mut server = Server {
        host: Host::new(),
        cells: Vec::new(),
        decoder: Decoder::new(),
        items: Vec::new(),
    };
    // Cells may talk to the host's network as soon as they start.
    server.host.wire_address(10_000_000_000);
    send(0, &Message::Hello { version: VERSION });
    let mut fast_until = 0;
    loop {
        server.read();
        let busy = !server.items.is_empty();
        for item in core::mem::take(&mut server.items) {
            if server.handle(item) {
                return;
            }
        }
        server.reap();
        server.flush();
        let now = now_ns();
        if busy {
            fast_until = now + FAST_FOR_NS;
        }
        let every = if now < fast_until {
            LOOK_FAST_NS
        } else {
            LOOK_SLOW_NS
        };
        let watched = server
            .cells
            .iter()
            .any(|e| !e.watchers.is_empty() && matches!(e.place, Place::Live(_)));
        let flush_at = now + OUTPUT_EVERY_NS;
        let Server {
            host,
            decoder,
            items,
            ..
        } = &mut server;
        host.look_every(Some(every));
        host.run(|h| {
            while let Some(byte) = link::read_byte() {
                if let Some(item) = decoder.push(byte) {
                    items.push(item);
                }
            }
            !items.is_empty()
                || decoder.in_frame()
                || h.has_finished()
                || (watched && now_ns() >= flush_at)
        });
    }
}

impl Server {
    /// Reads what the host sent; a frame begun is read to its end.
    fn read(&mut self) {
        let mut last = now_ns();
        loop {
            match link::read_byte() {
                Some(byte) => {
                    last = now_ns();
                    if let Some(item) = self.decoder.push(byte) {
                        self.items.push(item);
                    }
                }
                None if self.decoder.in_frame() && now_ns() - last < FRAME_GAP_NS => {
                    core::hint::spin_loop();
                }
                None => return,
            }
        }
    }

    /// Answers one frame; returns whether the host asked for the power off.
    fn handle(&mut self, item: Item) -> bool {
        let (id, request) = match item {
            Item::Text(_) => return false,
            Item::Bad { id, error } => {
                failed(id, 0, error, String::new());
                return false;
            }
            Item::Frame(frame) => (frame.id, Request::decode(&frame)),
        };
        let t0 = Counter.cycles();
        self.reap();
        let shutdown = request == Ok(Request::Shutdown);
        let outcome = match request {
            Ok(request) => self.execute(id, request),
            Err(error) => refuse(error),
        };
        let ns = timer::cycles_to_ns(Counter.cycles() - t0).unwrap_or(0);
        match outcome {
            Ok(body) => send(id, &Message::Done { ns, body }),
            Err((error, why)) => failed(id, ns, error, why),
        }
        shutdown
    }

    fn entry(&mut self, id: u32) -> Result<&mut Entry, (Error, String)> {
        self.cells
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or((Error::NoCell, String::new()))
    }

    fn live(&mut self, id: u32) -> Result<u32, (Error, String)> {
        match self.entry(id)?.place {
            Place::Live(cell) => Ok(cell),
            _ => refuse(Error::NotRunning),
        }
    }

    fn execute(&mut self, id: u32, request: Request) -> Outcome {
        match request {
            Request::Run(run) => self.run(run),
            Request::List => Ok(Body::List(
                self.cells
                    .iter()
                    .map(|e| CellInfo {
                        id: e.id,
                        image: e.image.clone(),
                        state: state_of(&e.place),
                    })
                    .collect(),
            )),
            Request::Suspend(target) => {
                let cell = self.live(target)?;
                let snapshot = self
                    .host
                    .suspend(cell)
                    .ok_or((Error::NoMemory, String::from("the snapshot does not fit")))?;
                let bytes = snapshot.as_bytes().len() as u64;
                self.reap();
                self.entry(target)?.place = Place::Suspended(snapshot);
                Ok(Body::Bytes(bytes))
            }
            Request::Resume(target) => {
                let entry = self.entry(target)?;
                let snapshot = match core::mem::replace(&mut entry.place, Place::Ended(State::Killed))
                {
                    Place::Suspended(snapshot) => snapshot,
                    other => {
                        entry.place = other;
                        return refuse(Error::NotSuspended);
                    }
                };
                match self.host.resume(snapshot, Resume::Lazy, true, false) {
                    Ok(cell) => {
                        let entry = self.entry(target)?;
                        (entry.place, entry.cell) = (Place::Live(cell), cell);
                        Ok(Body::None)
                    }
                    Err(snapshot) => {
                        self.entry(target)?.place = Place::Suspended(snapshot);
                        Err((Error::NoMemory, String::from("the cell does not fit")))
                    }
                }
            }
            Request::Fork { cell: target, n } => {
                if n == 0 || n > MAX_FORK {
                    return Err((Error::Failed, format!("from 1 to {MAX_FORK} children")));
                }
                let cell = self.live(target)?;
                let children = self
                    .host
                    .fork(cell, n as usize)
                    .ok_or((Error::NoMemory, String::from("the children do not fit")))?;
                let image = self.entry(target)?.image.clone();
                for &child in &children {
                    self.cells.push(Entry {
                        id: child,
                        image: image.clone(),
                        place: Place::Live(child),
                        cell: child,
                        out: Vec::new(),
                        watchers: Vec::new(),
                    });
                }
                Ok(Body::Cells(children))
            }
            Request::Kill(target) => {
                let entry = self.entry(target)?;
                match core::mem::replace(&mut entry.place, Place::Ended(State::Killed)) {
                    Place::Live(cell) => {
                        self.host.kill(cell);
                        self.reap();
                    }
                    Place::Suspended(snapshot) => cell::close_parked(&snapshot),
                    // Killing a cell that ended forgets it.
                    Place::Ended(_) => self.cells.retain(|e| e.id != target),
                }
                Ok(Body::None)
            }
            Request::Output(target) => {
                self.entry(target)?.watchers.push(id);
                Ok(Body::None)
            }
            Request::WriteFile { cell, path, data } => {
                let cell = self.live(cell)?;
                if self.host.write_file(cell, &path, &data) {
                    Ok(Body::None)
                } else {
                    Err((Error::Failed, String::from("cannot write the file")))
                }
            }
            Request::Stats => Ok(Body::Stats(self.stats())),
            Request::Shutdown => Ok(Body::None),
        }
    }

    fn run(&mut self, run: Run) -> Outcome {
        let image = cell::find_image(&run.image)
            .filter(|i| !i.is_empty())
            .ok_or((Error::NoImage, String::from_utf8_lossy(&run.image).into()))?;
        let mut files = match run.files_from.as_slice() {
            b"" => Some(cell::no_files()),
            b"cpython-files" => cell::cpython_files(),
            b"dynamic-files" => cell::dynamic_files(),
            _ => None,
        }
        .ok_or((
            Error::NoImage,
            String::from_utf8_lossy(&run.files_from).into(),
        ))?;
        for (path, data) in &run.files {
            files.put_file(path, data).map_err(|e| {
                (
                    Error::Failed,
                    format!("{}: {e:?}", String::from_utf8_lossy(path)),
                )
            })?;
        }
        let argv: Vec<&[u8]> = run.argv.iter().map(Vec::as_slice).collect();
        let envp: Vec<&[u8]> = run.envp.iter().map(Vec::as_slice).collect();
        let id = self
            .host
            .spawn_with_env(image, &argv, &envp, files, true, false)
            .map_err(|e| (Error::Failed, format!("{e:?}")))?;
        self.cells.push(Entry {
            id,
            image: run.image,
            place: Place::Live(id),
            cell: id,
            out: Vec::new(),
            watchers: Vec::new(),
        });
        Ok(Body::Cell(id))
    }

    fn stats(&self) -> Stats {
        let mut stats = Stats::default();
        for e in &self.cells {
            match &e.place {
                Place::Live(_) => stats.running += 1,
                Place::Suspended(s) => {
                    stats.suspended += 1;
                    stats.snapshot_bytes += s.as_bytes().len() as u64;
                }
                Place::Ended(_) => stats.ended += 1,
            }
        }
        let (total, free) =
            memory::with_frames(|f| (f.usable_frames() as u64, f.free_frames() as u64))
                .unwrap_or((0, 0));
        stats.memory_total = total * FRAME_SIZE;
        stats.memory_free = free * FRAME_SIZE;
        stats
    }

    /// Takes the cells that ended (or were suspended) from the kernel: their
    /// last output, and how they ended.
    fn reap(&mut self) {
        for finished in self.host.take_finished() {
            let Some(entry) = self.cells.iter_mut().find(|e| e.cell == finished.id) else {
                continue;
            };
            keep(&mut entry.out, &finished.output);
            if finished.ending != Ending::Suspended {
                entry.place = Place::Ended(match finished.ending {
                    Ending::Exit(code) => State::Exited(code),
                    Ending::Fault => State::Fault,
                    Ending::Killed | Ending::Suspended => State::Killed,
                });
            }
        }
    }

    /// Sends watched cells' new output, and tells their watchers when they
    /// ended.
    fn flush(&mut self) {
        for entry in &mut self.cells {
            if entry.watchers.is_empty() {
                continue;
            }
            if let Place::Live(cell) = entry.place
                && let Some(new) = self.host.take_output(cell)
            {
                keep(&mut entry.out, &new);
            }
            if !entry.out.is_empty() {
                let message = Message::Output {
                    cell: entry.id,
                    data: core::mem::take(&mut entry.out),
                };
                for &w in &entry.watchers {
                    send(w, &message);
                }
            }
            if let Place::Ended(state) = entry.place {
                for w in entry.watchers.drain(..) {
                    send(
                        w,
                        &Message::Ended {
                            cell: entry.id,
                            state,
                        },
                    );
                }
            }
        }
    }
}

fn failed(id: u32, ns: u64, error: Error, why: String) {
    send(
        id,
        &Message::Failed {
            ns,
            error,
            message: why.into_bytes(),
        },
    );
}

fn state_of(place: &Place) -> State {
    match place {
        Place::Live(_) => State::Running,
        Place::Suspended(s) => State::Suspended {
            bytes: s.as_bytes().len() as u64,
        },
        Place::Ended(state) => *state,
    }
}

/// Appends `new` to `out`, keeping at most [`KEPT_OUTPUT`] bytes (the
/// newest).
fn keep(out: &mut Vec<u8>, new: &[u8]) {
    out.extend_from_slice(new);
    if out.len() > KEPT_OUTPUT {
        out.drain(..out.len() - KEPT_OUTPUT);
    }
}
