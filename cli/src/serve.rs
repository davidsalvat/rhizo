//! The background server `rhizo up` leaves running (design D5 of
//! `add-control-plane`): it owns the VMM, and the control channel to its
//! kernel (the VM's vsock, else the serial line on the VMM's standard input
//! and output: change `control-over-vsock`), and relays frames between the
//! kernel and the clients of its Unix socket, giving each request an
//! identifier of its own so that replies and output go back to the client
//! that asked. The kernel's console text goes to the console log. Frames
//! keep the protocol version they came in, so a kernel of the version
//! before is relayed as it speaks (design D20 of `add-cell-hierarchy`).
//! The kernel's audit records go to the audit log, synced every second and
//! when the VM ends (design D18).

use crate::args::Up;
use crate::audit::Log;
use crate::state::{Dir, Info};
use rhizo_control::{Body, Decoder, Item, Message, PREVIOUS, Request, VERSION, encode_frame_as};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use xtask::control::{Event, Vm, Writer};
use xtask::vmm::Vmm;

/// Where a reply goes back.
struct Route {
    client: Arc<Mutex<UnixStream>>,
    /// The client's identifier for the request.
    id: u32,
    /// An `output` request, or a start that watches its cells (protocol
    /// version 4): its route lasts until this many cells ended (for a
    /// start, set by its reply).
    ends: Option<usize>,
}

/// How many cells' ends a request streams: one for `output`, those of its
/// reply for a watching `run` or `fork` (`None`: it streams nothing).
fn streams(request: &Request) -> Option<usize> {
    match request {
        Request::Output(_) => Some(1),
        Request::Run(run) if run.watch => Some(0),
        Request::Fork { watch: true, .. } => Some(0),
        Request::Scoped { request, .. } => streams(request),
        _ => None,
    }
}

#[derive(Default)]
struct Relay {
    routes: Mutex<HashMap<u32, Route>>,
    next: AtomicU32,
}

/// The VMM `up` asks for on the selected architecture.
pub fn vmm_of(up: &Up) -> Result<Vmm, String> {
    match &up.vmm {
        Some(name) => Vmm::parse(name),
        None => Ok(Vmm::default_for(up.arch)),
    }
}

/// Boots the VM and relays until it powers off.
pub fn serve(dir: &Dir, up: &Up) -> Result<(), String> {
    xtask::arch::select(up.arch);
    let vmm = vmm_of(up)?;
    let opts = xtask::control::run_options(up.accel, up.cpus, up.memory_mib);
    let mut console = File::create(dir.console())
        .map_err(|e| format!("cannot create {}: {e}", dir.console().display()))?;
    let t0 = Instant::now();
    let mut vm = Vm::start(vmm, &dir.kernel(), opts, Stdio::inherit())?;
    let boot_ms = t0.elapsed().as_millis() as u64;
    if vm.protocol != VERSION && vm.protocol != PREVIOUS {
        return Err(crate::client::upgrade(vm.protocol)); // dropping the VM ends it
    }
    let _ = console.write_all(&std::mem::take(&mut vm.console));
    let boot = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut audit = Log::open(&crate::audit::dir(&dir.0), boot)?;
    let _ = writeln!(console, "[rhizo] control frames over {}", vm.channel);
    let _ = fs::remove_file(dir.socket());
    let listener = UnixListener::bind(dir.socket())
        .map_err(|e| format!("cannot listen on {}: {e}", dir.socket().display()))?;
    // It may be outside the state directory (a long path): its user alone
    // connects, whatever the umask.
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir.socket(), fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("cannot restrict {}: {e}", dir.socket().display()))?;
    }
    Info {
        pid: std::process::id(),
        vmm_pid: vm.pid(),
        vmm: vmm.id().into(),
        arch: up.arch.id().into(),
        memory_mib: up.memory_mib,
        cpus: up.cpus,
        boot_ms,
        protocol: Some(vm.protocol),
    }
    .write(dir)?;
    let relay = Arc::new(Relay {
        next: AtomicU32::new(1),
        ..Relay::default()
    });
    {
        let (writer, relay) = (vm.writer(), Arc::clone(&relay));
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (writer, relay) = (writer.clone(), Arc::clone(&relay));
                thread::spawn(move || client(stream, &writer, &relay));
            }
        });
    }
    loop {
        let wait = if audit.unsynced() { 1 } else { 3600 };
        match vm.next(Duration::from_secs(wait)) {
            Some(Event::Text(text)) => {
                let _ = console.write_all(&text);
            }
            // Identifier 0: the kernel's own (an audit record).
            Some(Event::Frame(frame)) if frame.id == 0 => {
                if let Ok(Message::Audit(record)) = Message::decode(&frame)
                    && let Err(e) = audit.append(&record)
                {
                    let _ = writeln!(console, "[rhizo] audit record {}: {e}", record.seq);
                }
            }
            Some(Event::Frame(frame)) => relay.back(&frame),
            Some(Event::Bad { id, error }) => {
                let _ = writeln!(console, "[rhizo] unreadable frame {id}: {}", error.as_str());
            }
            Some(Event::Exited) => break,
            None => {}
        }
        audit.sync_due();
    }
    audit.sync();
    dir.clean();
    Ok(())
}

impl Relay {
    /// Sends a kernel frame back to the client whose request it answers.
    fn back(&self, frame: &rhizo_control::Frame) {
        let mut routes = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        let Some(route) = routes.get_mut(&frame.id) else {
            return;
        };
        let last = match (Message::decode(frame), &mut route.ends) {
            (Ok(Message::Done { .. }), None) => true,
            // A watching start: the cells it made are to end.
            (Ok(Message::Done { body, .. }), Some(ends @ 0)) => {
                *ends = match body {
                    Body::Cell(_) => 1,
                    Body::Cells(cells) => cells.len(),
                    _ => 0,
                };
                *ends == 0
            }
            (Ok(Message::Done { .. } | Message::Output { .. }), Some(_)) => false,
            (Ok(Message::Ended { .. }), Some(ends)) => {
                *ends = ends.saturating_sub(1);
                *ends == 0
            }
            _ => true,
        };
        let sent = route
            .client
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .write_all(&encode_frame_as(
                frame.version,
                frame.kind,
                route.id,
                &frame.payload,
            ))
            .is_ok();
        if last || !sent {
            routes.remove(&frame.id);
        }
    }
}

/// Relays one client's requests to the kernel.
fn client(stream: UnixStream, vm: &Writer, relay: &Relay) {
    let Ok(writer) = stream.try_clone() else {
        return;
    };
    let writer = Arc::new(Mutex::new(writer));
    let mut reader = stream;
    let mut decoder = Decoder::any_version();
    let mut buf = [0u8; 4096];
    while let Ok(n) = reader.read(&mut buf) {
        if n == 0 {
            return;
        }
        for &byte in &buf[..n] {
            match decoder.push(byte) {
                Some(Item::Frame(frame)) => {
                    let id = relay.next.fetch_add(1, Ordering::Relaxed);
                    let ends = Request::decode(&frame).ok().as_ref().and_then(streams);
                    relay
                        .routes
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(
                            id,
                            Route {
                                client: Arc::clone(&writer),
                                id: frame.id,
                                ends,
                            },
                        );
                    if vm
                        .send(&encode_frame_as(
                            frame.version,
                            frame.kind,
                            id,
                            &frame.payload,
                        ))
                        .is_err()
                    {
                        return;
                    }
                }
                Some(Item::Bad { id, error }) => {
                    let refusal = Message::Failed {
                        ns: 0,
                        error,
                        message: Vec::new(),
                    };
                    let _ = writer
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .write_all(&refusal.encode(id));
                }
                Some(Item::Text(_)) | None => {}
            }
        }
    }
}
