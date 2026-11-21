//! The background server `rhizo up` leaves running (design D5 of
//! `add-control-plane`): it owns the VMM, and the control channel to its
//! kernel (the VM's vsock, else the serial line on the VMM's standard input
//! and output: change `control-over-vsock`), and relays frames between the
//! kernel and the clients of its Unix socket, giving each request an
//! identifier of its own so that replies and output go back to the client
//! that asked. The kernel's console text goes to the console log.

use crate::args::Up;
use crate::state::{Dir, Info};
use rhizo_control::{Decoder, Item, Message, Request, encode_frame};
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
    /// An `output` request: its route lasts until the cell ends.
    stream: bool,
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
    let _ = console.write_all(&std::mem::take(&mut vm.console));
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
        match vm.next(Duration::from_secs(3600)) {
            Some(Event::Text(text)) => {
                let _ = console.write_all(&text);
            }
            Some(Event::Frame(frame)) => relay.back(frame.kind, frame.id, &frame.payload),
            Some(Event::Bad { id, error }) => {
                let _ = writeln!(console, "[rhizo] unreadable frame {id}: {}", error.as_str());
            }
            Some(Event::Exited) => break,
            None => {}
        }
    }
    dir.clean();
    Ok(())
}

impl Relay {
    /// Sends a kernel frame back to the client whose request it answers.
    fn back(&self, kind: u8, id: u32, payload: &[u8]) {
        let mut routes = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        let Some(route) = routes.get(&id) else {
            return;
        };
        let last = match Message::decode(&rhizo_control::Frame {
            kind,
            id,
            payload: payload.to_vec(),
        }) {
            Ok(Message::Done { .. }) => !route.stream,
            Ok(Message::Output { .. }) => false,
            _ => true,
        };
        let sent = route
            .client
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .write_all(&encode_frame(kind, route.id, payload))
            .is_ok();
        if last || !sent {
            routes.remove(&id);
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
    let mut decoder = Decoder::new();
    let mut buf = [0u8; 4096];
    while let Ok(n) = reader.read(&mut buf) {
        if n == 0 {
            return;
        }
        for &byte in &buf[..n] {
            match decoder.push(byte) {
                Some(Item::Frame(frame)) => {
                    let id = relay.next.fetch_add(1, Ordering::Relaxed);
                    let stream = matches!(Request::decode(&frame), Ok(Request::Output(_)));
                    relay
                        .routes
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(
                            id,
                            Route {
                                client: Arc::clone(&writer),
                                id: frame.id,
                                stream,
                            },
                        );
                    if vm
                        .send(&encode_frame(frame.kind, id, &frame.payload))
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
