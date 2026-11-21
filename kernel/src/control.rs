//! The control kernel (spec `control-plane`, change `add-control-plane`):
//! after booting it serves the host's requests on the serial line, the
//! console's own, or, once the host connects to its virtio socket device,
//! on that stream (change `control-over-vsock`), instead of running tests
//! or benchmarks. It only does what
//! it is asked: which programs, with which files and when to suspend, fork
//! or kill them is the host's business (`rhizo`, design D1).
//!
//! A cell keeps its identifier for its whole life here, and identifiers
//! count up by one for each cell the host starts, forks or loads: the
//! kernel's own cell identifiers, which a resume also takes, stay behind
//! this table (change `fix-cli-nits`).
//!
//! Snapshots leave and enter as bytes (change `snapshots-on-disk`): a
//! save sends a suspended cell's or a template's exported snapshot to the
//! host over TCP through the network card, and a load fetches one the same
//! way, then validates it as the untrusted input it is
//! (`Snapshot::from_bytes`).

use crate::arch::timer::{self, Counter};
use crate::cell::{self, Ending, Host, Resume, now_ns};
use crate::memory;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use rhizo_cell::quota::Quota;
use rhizo_control::{
    Body, CellInfo, Decoder, Error, Item, Message, Request, Run, State, Stats, VERSION,
};
use rhizo_hal::CycleCounter;
use rhizo_mm::FRAME_SIZE;
use rhizo_snapshot::{FormatError, Snapshot};

/// How often the boot CPU looks at the control channel while it has
/// nothing to run: often right after a request, less when the host is quiet
/// (design D4).
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
/// Longest the kernel waits, before the power off, for its last frames
/// to leave (`link::finish`).
const FINISH_NS: u64 = 1_000_000_000;
/// Most cells one fork makes.
const MAX_FORK: u32 = 1024;

/// Where a cell is.
enum Place {
    /// Running as this kernel cell.
    Live(u32),
    Suspended(Snapshot),
    /// A template, as this kernel template.
    Template(u32),
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
    /// The identifier of the next cell the host starts or forks.
    next: u32,
    decoder: Decoder,
    /// Frames and refusals read, not yet handled.
    items: Vec<Item>,
    /// Whether frames go over the vsock stream (else the serial line).
    on_vsock: bool,
}

/// The control channel: the vsock stream while the host is connected to
/// it, the serial line otherwise (design D2 of `control-over-vsock`).
mod link {
    use crate::arch::console::control as serial;
    use crate::vsock;

    /// Whether the host is connected over vsock (the device is polled).
    pub fn on_vsock() -> bool {
        vsock::poll()
    }

    /// The next byte the host sent, if one is waiting.
    pub fn read_byte() -> Option<u8> {
        if vsock::poll() {
            vsock::read_byte()
        } else {
            serial::read_byte()
        }
    }

    /// Sends `bytes` (a frame) to the host as they are.
    pub fn write(bytes: &[u8]) {
        if !vsock::write(bytes) {
            serial::write(bytes);
        }
    }

    /// Sends what is still queued for the host before the power off,
    /// waiting at most [`FINISH_NS`](super::FINISH_NS): a frame queued on
    /// the vsock connection, or in the device, would be lost with the VM.
    pub fn finish() {
        vsock::finish(crate::cell::now_ns() + super::FINISH_NS);
    }
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
        next: 1,
        decoder: Decoder::new(),
        items: Vec::new(),
        on_vsock: false,
    };
    // Cells may talk to the host's network as soon as they start.
    server.host.wire_address(10_000_000_000);
    crate::vsock::init();
    send(0, &Message::Hello { version: VERSION });
    let mut fast_until = 0;
    loop {
        server.read();
        let busy = !server.items.is_empty();
        for item in core::mem::take(&mut server.items) {
            if server.handle(item) {
                // The reply to the power off goes out before the VM does.
                link::finish();
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
        // Watched cells' output goes out as soon as they wait for something
        // (an agent's reply, then its read of the next line), else every
        // `OUTPUT_EVERY_NS` (design D3 of `add-interactive-chat`).
        let watched: Vec<u32> = server
            .cells
            .iter()
            .filter(|e| !e.watchers.is_empty())
            .filter_map(|e| match e.place {
                Place::Live(cell) => Some(cell),
                _ => None,
            })
            .collect();
        let flush_at = now + OUTPUT_EVERY_NS;
        let Server {
            host,
            decoder,
            items,
            on_vsock,
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
                || crate::vsock::connected() != *on_vsock
                || h.has_finished()
                || (!watched.is_empty() && now_ns() >= flush_at)
                || watched.iter().any(|&cell| h.has_output(cell))
        });
    }
}

impl Server {
    /// Reads what the host sent; a frame begun is read to its end. A new
    /// vsock connection is greeted, and frames go there from then on.
    fn read(&mut self) {
        let on_vsock = link::on_vsock();
        if on_vsock != self.on_vsock {
            self.on_vsock = on_vsock;
            self.decoder = Decoder::new();
            if on_vsock {
                send(0, &Message::Hello { version: VERSION });
            }
        }
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
                let bytes = snapshot.heap_bytes() as u64;
                self.reap();
                self.entry(target)?.place = Place::Suspended(snapshot);
                Ok(Body::Bytes(bytes))
            }
            Request::Resume(target) => {
                let entry = self.entry(target)?;
                let snapshot =
                    match core::mem::replace(&mut entry.place, Place::Ended(State::Killed)) {
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
                let children = match self.entry(target)?.place {
                    Place::Template(template) => self.copies_of(template, n),
                    _ => {
                        let cell = self.live(target)?;
                        self.host.fork(cell, n as usize)
                    }
                }
                .ok_or((Error::NoMemory, String::from("the children do not fit")))?;
                let image = self.entry(target)?.image.clone();
                let ids = children
                    .into_iter()
                    .map(|child| self.add(image.clone(), child))
                    .collect();
                Ok(Body::Cells(ids))
            }
            Request::Kill(target) => {
                let entry = self.entry(target)?;
                match core::mem::replace(&mut entry.place, Place::Ended(State::Killed)) {
                    Place::Live(cell) => {
                        self.host.kill(cell);
                        self.reap();
                    }
                    Place::Suspended(snapshot) => {
                        cell::close_parked(&snapshot);
                        self.host.drop_snapshot(&snapshot);
                    }
                    Place::Template(template) => {
                        self.host.drop_template(template);
                    }
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
            Request::Input { cell, data } => self.input(cell, Some(&data)),
            Request::CloseInput(cell) => self.input(cell, None),
            Request::Freeze(target) => {
                let cell = self.live(target)?;
                let out = self.host.take_output(cell).unwrap_or_default();
                if !self.host.freeze(cell) {
                    return refuse(Error::NotRunning);
                }
                let entry = self.entry(target)?;
                keep(&mut entry.out, &out);
                entry.place = Place::Template(cell);
                Ok(Body::None)
            }
            Request::Save { cell, host } => self.save(cell, host),
            Request::Load {
                size,
                template,
                host,
            } => self.load(size, template, host),
        }
    }

    /// Cells made from template `template`, all or none.
    fn copies_of(&mut self, template: u32, n: u32) -> Option<Vec<u32>> {
        let mut children = Vec::new();
        for _ in 0..n {
            match self.host.spawn_from_template(template, true, false) {
                Some(child) => children.push(child),
                None => {
                    for child in children {
                        self.host.kill(child);
                    }
                    self.reap();
                    return None;
                }
            }
        }
        Some(children)
    }

    /// Sends the snapshot of suspended cell or template `target` to the
    /// host (design D2 of `snapshots-on-disk`).
    fn save(&mut self, target: u32, to: rhizo_control::Host) -> Outcome {
        let bytes = match &self.entry(target)?.place {
            Place::Suspended(snapshot) => {
                let size = snapshot.heap_bytes() as u64 + snapshot.shared_bytes();
                if !cell::cells_may_take(size) {
                    return Err((Error::NoMemory, String::from("the file does not fit")));
                }
                snapshot.export(&crate::memory::DirectMap, crate::arch::mem::crc32c)
            }
            &Place::Template(template) => self
                .host
                .template_snapshot(template)
                .ok_or((Error::NoMemory, String::from("the file does not fit")))?
                .export(&crate::memory::DirectMap, crate::arch::mem::crc32c),
            _ => {
                return Err((
                    Error::NotSuspended,
                    String::from("only a suspended cell or a template is saved"),
                ));
            }
        };
        self.host
            .send_bytes(to.ip, to.port, to.token, &bytes)
            .map_err(transfer)?;
        Ok(Body::Bytes(bytes.len() as u64))
    }

    /// Fetches a snapshot of `size` bytes from the host, validates it and
    /// makes it a suspended cell or a template under the next identifier
    /// (design D2 to D4 of `snapshots-on-disk`).
    fn load(&mut self, size: u64, template: bool, from: rhizo_control::Host) -> Outcome {
        let size = usize::try_from(size).unwrap_or(usize::MAX);
        let mut bytes = Vec::new();
        if !cell::cells_may_take(size as u64) || bytes.try_reserve_exact(size).is_err() {
            return Err((Error::NoMemory, format!("{size} bytes do not fit")));
        }
        self.host
            .fetch_bytes(from.ip, from.port, from.token, size, &mut bytes)
            .map_err(transfer)?;
        let mut snapshot =
            Snapshot::from_bytes(bytes, crate::arch::user::MACHINE, crate::arch::mem::crc32c)
                .map_err(|e| (Error::BadSnapshot, describe(e)))?;
        // It gets no more than a cell the host starts (design D4).
        snapshot.cap_quota(Quota::DEFAULT);
        let place = if template {
            let t = self
                .host
                .load_template(snapshot)
                .map_err(|_| (Error::NoMemory, String::from("the template does not fit")))?;
            Place::Template(t)
        } else {
            Place::Suspended(snapshot)
        };
        let id = self.next;
        self.next += 1;
        self.cells.push(Entry {
            id,
            image: b"loaded".to_vec(),
            place,
            cell: 0,
            out: Vec::new(),
            watchers: Vec::new(),
        });
        Ok(Body::Cell(id))
    }

    /// Writes into a cell's standard input, or closes it (design D1 of
    /// `add-interactive-chat`).
    fn input(&mut self, target: u32, data: Option<&[u8]>) -> Outcome {
        let cell = self.live(target)?;
        match self.host.input(cell, data) {
            Some(Ok(())) => Ok(Body::None),
            Some(Err(e)) => Err(match e {
                rhizo_fs::Error::BadFd => (
                    Error::Failed,
                    String::from("the cell has no input from the host, or it was closed"),
                ),
                rhizo_fs::Error::Pipe => (
                    Error::Failed,
                    String::from("the cell closed its standard input"),
                ),
                rhizo_fs::Error::Again => (
                    Error::Failed,
                    format!(
                        "the cell's input is full ({} KiB unread)",
                        rhizo_fs::PIPE_CAP >> 10
                    ),
                ),
                _ => (Error::NoMemory, String::new()),
            }),
            None => refuse(Error::NotRunning),
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
        if run.input {
            files
                .host_input()
                .map_err(|e| (Error::Failed, format!("standard input: {e:?}")))?;
        }
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
        let cell = self
            .host
            .spawn_with_env(image, &argv, &envp, files, true, false)
            .map_err(|e| (Error::Failed, format!("{e:?}")))?;
        Ok(Body::Cell(self.add(run.image, cell)))
    }

    /// Enters kernel cell `cell`, running `image`, in the table under the
    /// next identifier, and returns it.
    fn add(&mut self, image: Vec<u8>, cell: u32) -> u32 {
        let id = self.next;
        self.next += 1;
        self.cells.push(Entry {
            id,
            image,
            place: Place::Live(cell),
            cell,
            out: Vec::new(),
            watchers: Vec::new(),
        });
        id
    }

    fn stats(&self) -> Stats {
        let mut stats = Stats::default();
        for e in &self.cells {
            match &e.place {
                Place::Live(_) => stats.running += 1,
                Place::Suspended(s) => {
                    stats.suspended += 1;
                    stats.snapshot_bytes += s.heap_bytes() as u64;
                }
                Place::Template(_) => stats.templates += 1,
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
                // A suspended cell ends when its parent's end cascades.
                if let Place::Suspended(snapshot) = &entry.place {
                    cell::close_parked(snapshot);
                }
                entry.place = Place::Ended(match finished.ending {
                    Ending::Exit(code) => State::Exited(code),
                    Ending::Fault => State::Fault,
                    Ending::Killed | Ending::OutOfCpu | Ending::Suspended => State::Killed,
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
            bytes: s.heap_bytes() as u64,
        },
        Place::Template(_) => State::Template,
        Place::Ended(state) => *state,
    }
}

/// A failed transfer of snapshot bytes, for people.
fn transfer(e: cell::Transfer) -> (Error, String) {
    let why = match e {
        cell::Transfer::Net(rhizo_net::Error::NetUnreach) => {
            String::from("the VM has no network card with an address: it carries snapshot files")
        }
        cell::Transfer::Net(e) => format!("the host's end of the transfer: {e:?}"),
        cell::Transfer::Short => String::from("the host sent fewer bytes than it announced"),
        cell::Transfer::Stalled => String::from("the transfer stalled"),
    };
    (Error::Failed, why)
}

/// Why bytes are not a snapshot this kernel loads, for people (spec
/// `snapshots`, files).
fn describe(e: FormatError) -> String {
    let arch = |m: u16| match m {
        62 => "x86_64",
        183 => "aarch64",
        _ => "an unknown architecture",
    };
    match e {
        FormatError::Truncated => String::from("truncated"),
        FormatError::BadMagic => String::from("not a Rhizo snapshot"),
        FormatError::Version(v) => format!(
            "snapshot format version {v}; this kernel reads versions {} and {}{}",
            rhizo_snapshot::PREVIOUS,
            rhizo_snapshot::VERSION,
            if v < rhizo_snapshot::PREVIOUS {
                " (convert it with `rhizo snapshot upgrade`)"
            } else {
                ""
            }
        ),
        FormatError::Machine { expected, found } => format!(
            "taken on {}; this kernel runs on {}",
            arch(found),
            arch(expected)
        ),
        FormatError::Checksum => {
            String::from("checksum mismatch: the file is damaged or truncated")
        }
        other => format!("invalid snapshot: {other:?}"),
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
