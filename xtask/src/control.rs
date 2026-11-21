//! A control kernel on a VMM (spec `control-plane`, design D2 of
//! `add-control-plane`): the VMM's standard output carries the kernel's
//! console text; requests and replies go over the VM's virtio socket device
//! where it has one (change `control-over-vsock`), else over the serial
//! line, the VMM's standard input and output. `rhizo up` keeps one of these
//! behind a socket; the benchmark times requests through one; the `control`
//! test step drives `rhizo` itself against one.

use crate::bench::{Metric, summarize};
use crate::report::Failure;
use crate::vmm::{self, Accel, Guest, RunOptions, Vmm};
use crate::{Profile, Result, arch, build_kernel, report, target_dir, workspace_root};
use rhizo_control::{Body, Decoder, Frame, Item, Message, Request, Run, State, VSOCK_PORT};
use std::io::{self as stdio, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// How long a control kernel may take to boot and say hello.
pub const BOOT_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a booted control kernel's vsock may take to answer.
const VSOCK_TIMEOUT: Duration = Duration::from_secs(10);

/// The line the chat agent (`cells/python/agent_chat.py`, change
/// `add-interactive-chat`) prints once it reads its input.
pub const CHAT_READY: &str = "\x1eready";

/// The line the chat agent prints after each answer: the time it waited
/// for the model, and the whole turn in the cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatTurn {
    pub model_ns: u64,
    pub agent_ns: u64,
}

impl ChatTurn {
    /// The turn `line` (`\x1eturn model_ns=M agent_ns=A`) reports, if it is
    /// one.
    pub fn parse(line: &str) -> Option<Self> {
        let mut fields = line.strip_prefix("\x1eturn ")?.split(' ');
        let mut field = |name: &str| {
            fields
                .next()?
                .strip_prefix(name)?
                .strip_prefix('=')?
                .parse()
                .ok()
        };
        let turn = ChatTurn {
            model_ns: field("model_ns")?,
            agent_ns: field("agent_ns")?,
        };
        fields.next().is_none().then_some(turn)
    }
}

/// What the VM sent.
#[derive(Debug)]
pub enum Event {
    /// Console text.
    Text(Vec<u8>),
    /// A frame (a reply or an output event).
    Frame(Frame),
    /// A frame the host could not read.
    Bad {
        id: u32,
        error: rhizo_control::Error,
    },
    /// The VMM ended.
    Exited,
}

/// Where frames go: the serial line, or the vsock stream once connected.
type Link = Box<dyn Write + Send>;

/// Sends frames to a VM, from any thread.
#[derive(Clone)]
pub struct Writer(Arc<Mutex<Link>>);

impl std::fmt::Debug for Writer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Writer")
    }
}

impl Writer {
    /// Sends bytes (whole frames) to the kernel.
    pub fn send(&self, bytes: &[u8]) -> Result {
        let mut link = self.0.lock().unwrap_or_else(|e| e.into_inner());
        link.write_all(bytes)
            .and_then(|()| link.flush())
            .map_err(|e| format!("cannot write to the VM: {e}"))
    }
}

/// A running control kernel.
#[derive(Debug)]
pub struct Vm {
    child: Child,
    stdin: Writer,
    events: Receiver<Event>,
    /// Console text seen by [`call`](Self::call), for error reports.
    pub console: Vec<u8>,
    next_id: u32,
    /// The VM's socket device, if it has one.
    vsock: Option<vmm::Vsock>,
    /// Where the frames go, for people: `vsock`, or the serial line and
    /// why.
    pub channel: String,
}

/// Decodes what `from` carries into `tx` until it ends; then, if
/// `vmm_output`, tells that the VMM ended.
fn read_frames(mut from: impl Read, tx: &Sender<Event>, vmm_output: bool) {
    let mut decoder = Decoder::new();
    let mut buf = [0u8; 4096];
    while let Ok(n) = from.read(&mut buf) {
        if n == 0 {
            break;
        }
        let mut text = Vec::new();
        for &byte in &buf[..n] {
            match decoder.push(byte) {
                Some(Item::Text(b)) => text.push(b),
                Some(Item::Frame(frame)) => {
                    if !text.is_empty() {
                        let _ = tx.send(Event::Text(std::mem::take(&mut text)));
                    }
                    let _ = tx.send(Event::Frame(frame));
                }
                Some(Item::Bad { id, error }) => {
                    let _ = tx.send(Event::Bad { id, error });
                }
                None => {}
            }
        }
        if !text.is_empty() {
            let _ = tx.send(Event::Text(text));
        }
    }
    if vmm_output {
        let _ = tx.send(Event::Exited);
    }
}

/// Connects to the control kernel's port on `vsock`, retrying while the
/// VMM or the kernel is not ready, for at most [`VSOCK_TIMEOUT`].
fn connect(vsock: vmm::Vsock) -> stdio::Result<(Box<dyn Read + Send>, Link)> {
    let deadline = Instant::now() + VSOCK_TIMEOUT;
    loop {
        let attempt = match vsock {
            vmm::Vsock::Unix { .. } => connect_unix(&vsock.socket()),
            vmm::Vsock::Vhost { cid } => connect_vhost(cid).and_then(|f| {
                let reader: Box<dyn Read + Send> = Box::new(f.try_clone()?);
                Ok((reader, Box::new(f) as Link))
            }),
        };
        match attempt {
            Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            result => return result,
        }
    }
}

/// Firecracker's and Cloud Hypervisor's vsock: a Unix socket where
/// `CONNECT <port>` asks for a connection to the guest's port, and
/// `OK <host port>` says it is made.
fn connect_unix(path: &Path) -> stdio::Result<(Box<dyn Read + Send>, Link)> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(VSOCK_TIMEOUT))?;
    stream.write_all(format!("CONNECT {VSOCK_PORT}\n").as_bytes())?;
    // A byte at a time: the guest's bytes follow the line.
    let mut line = Vec::new();
    let mut byte = [0u8];
    while line.len() < 64 {
        stream.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
    }
    if !line.starts_with(b"OK ") {
        let line = String::from_utf8_lossy(&line).into_owned();
        return Err(stdio::Error::other(format!("the VMM answered {line:?}")));
    }
    stream.set_read_timeout(None)?;
    Ok((Box::new(stream.try_clone()?), Box::new(stream)))
}

/// QEMU's `vhost-vsock`: an `AF_VSOCK` socket to the guest's port.
fn connect_vhost(cid: u32) -> stdio::Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    // SAFETY: socket(2) with constant arguments; the result is checked.
    let fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(stdio::Error::last_os_error());
    }
    // SAFETY: `fd` is a descriptor just opened, which nothing else owns.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: all-zero bytes are a valid `sockaddr_vm` (plain integers).
    let mut addr: libc::sockaddr_vm = unsafe { std::mem::zeroed() };
    addr.svm_family = libc::AF_VSOCK as libc::sa_family_t;
    addr.svm_cid = cid;
    addr.svm_port = VSOCK_PORT;
    // SAFETY: `addr` is a `sockaddr_vm` alive across the call, whose size
    // is given; `fd` is an open socket.
    let r = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&raw const addr).cast(),
            size_of::<libc::sockaddr_vm>() as libc::socklen_t,
        )
    };
    if r < 0 {
        return Err(stdio::Error::last_os_error());
    }
    Ok(fd.into())
}

/// The run options of a control kernel on `vmm`: a network card where the
/// VMM allows it, `cpus` CPUs and `memory_mib` of memory.
pub fn run_options(accel: Accel, cpus: u32, memory_mib: u32) -> RunOptions {
    RunOptions {
        stop_at_ready: false,
        timeout: BOOT_TIMEOUT,
        echo: false,
        accel,
        gdb: false,
        network: true,
        pci_card: false,
        cpus,
        memory_mib,
        bench: None,
        shard: None,
        selfcheck: None,
        control: true,
        vsock: None,
    }
}

impl Vm {
    /// Boots `kernel` (a `control` build) on `vmm` and waits for its hello.
    /// The VMM's error output goes to `stderr`.
    pub fn start(vmm: Vmm, kernel: &Path, mut opts: RunOptions, stderr: Stdio) -> Result<Self> {
        if let Some(reason) = vmm.unavailable_reason(opts.accel) {
            return Err(format!("{} is not available: {reason}", vmm.id()));
        }
        let (vsock, channel) = match vmm::Vsock::for_vmm(vmm) {
            Ok(v) => (Some(v), String::from("vsock")),
            Err(why) => (None, format!("serial line ({why})")),
        };
        if let Some(v) = vsock {
            let _ = std::fs::remove_file(v.socket());
        }
        opts.vsock = vsock;
        let scratch = crate::tools::dir()?;
        let mut cmd: Command =
            vmm::command(vmm, &Guest::Rhizo(kernel.to_path_buf()), &opts, &scratch)?;
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr);
        if std::env::var_os("RHIZO_PRINT_VMM_COMMAND").is_some() {
            eprintln!("[rhizo] {cmd:?}");
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("cannot start {} (is it installed?): {e}", vmm.id()))?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let (tx, events) = mpsc::channel();
        let serial = tx.clone();
        thread::spawn(move || read_frames(stdout, &serial, true));
        let mut vm = Vm {
            child,
            stdin: Writer(Arc::new(Mutex::new(Box::new(stdin)))),
            events,
            console: Vec::new(),
            next_id: 1,
            vsock,
            channel,
        };
        vm.hello(vmm, BOOT_TIMEOUT, "on its serial line")?;
        if let Some(v) = vsock {
            // From now on the frames take the stream (design D3 of
            // `control-over-vsock`).
            let (reader, writer) = connect(v).map_err(|e| {
                format!(
                    "cannot connect to the control kernel's vsock on {}: {e}",
                    vmm.id()
                )
            })?;
            thread::spawn(move || read_frames(reader, &tx, false));
            *vm.stdin.0.lock().unwrap_or_else(|e| e.into_inner()) = writer;
            vm.hello(vmm, VSOCK_TIMEOUT, "on its vsock")?;
        }
        Ok(vm)
    }

    /// Waits at most `timeout` for the kernel's hello; on failure, ends the
    /// VMM.
    fn hello(&mut self, vmm: Vmm, timeout: Duration, channel: &str) -> Result {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.next(left) {
                Some(Event::Frame(f))
                    if matches!(Message::decode(&f), Ok(Message::Hello { .. })) =>
                {
                    return Ok(());
                }
                Some(Event::Text(t)) => self.console.extend(t),
                Some(Event::Exited) | None => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return Err(format!(
                        "the control kernel on {} did not say hello {channel}; its console:\n{}",
                        vmm.id(),
                        String::from_utf8_lossy(&self.console)
                    ));
                }
                Some(_) => {}
            }
        }
    }

    /// The VMM's process identifier.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// The next event, waiting at most `timeout`.
    pub fn next(&self, timeout: Duration) -> Option<Event> {
        match self.events.recv_timeout(timeout) {
            Ok(event) => Some(event),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Event::Exited),
        }
    }

    /// Sends bytes (whole frames) to the kernel.
    pub fn send(&self, bytes: &[u8]) -> Result {
        self.stdin.send(bytes)
    }

    /// A handle that sends to the VM from another thread.
    pub fn writer(&self) -> Writer {
        self.stdin.clone()
    }

    /// Sends `request` without waiting for its reply; returns its
    /// identifier.
    pub fn submit(&mut self, request: &Request) -> Result<u32> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&request.encode(id))?;
        Ok(id)
    }

    /// Sends `request` and waits for its reply (other frames are dropped,
    /// text kept in [`console`](Self::console)); returns the reply and the
    /// round trip as the host saw it.
    pub fn call(&mut self, request: &Request) -> Result<(Message, Duration)> {
        let t0 = Instant::now();
        let id = self.submit(request)?;
        loop {
            match self.next(Duration::from_secs(60)) {
                Some(Event::Frame(f)) if f.id == id => {
                    let took = t0.elapsed();
                    let message = Message::decode(&f)
                        .map_err(|e| format!("a reply the host cannot read: {}", e.as_str()))?;
                    return Ok((message, took));
                }
                Some(Event::Text(t)) => self.console.extend(t),
                Some(Event::Exited) | None => {
                    return Err(format!(
                        "no reply to {request:?}; console:\n{}",
                        String::from_utf8_lossy(&self.console)
                    ));
                }
                Some(_) => {}
            }
        }
    }

    /// Asks the kernel to power off and waits for the VMM to end.
    pub fn shutdown(mut self) {
        let _ = self.call(&Request::Shutdown);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Vm {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(v @ vmm::Vsock::Unix { .. }) = self.vsock {
            let _ = std::fs::remove_file(v.socket());
        }
    }
}

/// `call` that wants success.
fn done(vm: &mut Vm, request: &Request) -> Result<(Body, Duration)> {
    match vm.call(request)? {
        (Message::Done { body, .. }, took) => Ok((body, took)),
        (other, _) => Err(format!("{request:?} failed: {other:?}")),
    }
}

/// `List` round trips timed per boot.
const ROUND_TRIPS: usize = 200;
/// Rounds of run, suspend, resume, fork and kill per boot.
const OP_ROUNDS: usize = 20;

/// The control plane's figures on every VMM (spec `benchmarks`): a `list`
/// round trip from the host through the kernel and back, and each
/// operation on an idle cell as the host sees it.
pub fn measure(
    vmms: &[Vmm],
    accel: Accel,
    metrics: &mut Vec<Metric>,
    skipped: &mut Vec<(String, String)>,
) -> Result {
    let kernel = build_kernel(Profile::Release, &["control"])?;
    for &vmm in vmms {
        if vmm.unavailable_reason(accel).is_some() {
            continue; // already reported by the boot benchmark
        }
        report::text(&format!("control plane on {}", vmm.id()));
        match measure_on(vmm, &kernel, accel) {
            Ok(m) => metrics.extend(m),
            Err(e) => skipped.push((format!("control/{}", vmm.id()), e)),
        }
    }
    Ok(())
}

fn measure_on(vmm: Vmm, kernel: &Path, accel: Accel) -> Result<Vec<Metric>> {
    let opts = run_options(accel, 1, vmm::DEFAULT_MEMORY_MIB);
    let mut vm = Vm::start(vmm, kernel, opts, Stdio::null())?;
    report::text(&format!("  frames over the {}", vm.channel));
    let us = |d: Duration| d.as_secs_f64() * 1e6;
    let mut samples: Vec<(&str, Vec<f64>)> = vec![
        ("control.roundtrip_us", Vec::new()),
        ("control.run_us", Vec::new()),
        ("control.suspend_us", Vec::new()),
        ("control.resume_us", Vec::new()),
        ("control.fork_us", Vec::new()),
        ("control.kill_us", Vec::new()),
    ];
    for _ in 0..ROUND_TRIPS {
        samples[0].1.push(us(done(&mut vm, &Request::List)?.1));
    }
    let idle = Request::Run(Run {
        image: b"idle".to_vec(),
        argv: vec![b"idle".to_vec()],
        ..Run::default()
    });
    for _ in 0..OP_ROUNDS {
        let (Body::Cell(cell), took) = done(&mut vm, &idle)? else {
            return Err("`run` returned no cell".into());
        };
        samples[1].1.push(us(took));
        samples[2]
            .1
            .push(us(done(&mut vm, &Request::Suspend(cell))?.1));
        samples[3]
            .1
            .push(us(done(&mut vm, &Request::Resume(cell))?.1));
        let (Body::Cells(children), took) = done(&mut vm, &Request::Fork { cell, n: 1 })? else {
            return Err("`fork` returned no cells".into());
        };
        samples[4].1.push(us(took));
        samples[5]
            .1
            .push(us(done(&mut vm, &Request::Kill(cell))?.1));
        for id in children.into_iter().chain([cell]) {
            // Kill the child, then forget both.
            done(&mut vm, &Request::Kill(id))?;
            if id != cell {
                done(&mut vm, &Request::Kill(id))?;
            }
        }
    }
    samples.extend(first_outputs(&mut vm)?);
    samples.extend(serial_probes(&mut vm)?);
    let cpython = crate::cpython::root().join("usr/bin/python3").exists();
    if cpython {
        samples.push(("chat.turn_us", chat_turns(&mut vm, vmm)?));
    }
    let metric = |name: &str, unit: &str, mut values: Vec<f64>| {
        let runs = values.len();
        let (median, p90) = summarize(&mut values);
        Metric {
            name: name.into(),
            guest: "rhizo".into(),
            vmm: Some(vmm.id().into()),
            unit: unit.into(),
            median,
            p90,
            runs,
        }
    };
    let mut metrics: Vec<Metric> = samples
        .into_iter()
        .map(|(name, values)| {
            let unit = if name.ends_with("_ns_per_byte") {
                "ns"
            } else {
                "us"
            };
            metric(name, unit, values)
        })
        .collect();
    let qemu = matches!(vmm, Vmm::QemuVirt | Vmm::QemuMicrovm | Vmm::QemuQ35);
    if cpython && qemu {
        for (name, unit, value) in snapshot_files(vm, vmm, kernel, opts)? {
            metrics.push(metric(name, unit, vec![value]));
        }
    } else {
        vm.shutdown();
    }
    Ok(metrics)
}

/// Cells started per boot and timed to their first output.
const FIRST_OUTPUT_ROUNDS: usize = 20;

/// A cell started until the host had the first bytes it printed, as the
/// host saw it.
struct FirstOutput {
    /// From sending the request that starts it to the first output read.
    total: Duration,
    /// The start request's round trip.
    reply: Duration,
    /// The kernel's own time for that request.
    kernel: Duration,
    /// From sending the `output` request to the first output read.
    watch: Duration,
}

/// Starts one cell with `start` (`run`, or `fork` into one), streams its
/// output until the first bytes come, waits for its end and forgets it.
fn first_output(vm: &mut Vm, start: &Request) -> Result<FirstOutput> {
    let t0 = Instant::now();
    let (body, ns, reply) = match vm.call(start)? {
        (Message::Done { body, ns }, took) => (body, ns, took),
        (other, _) => return Err(format!("{start:?} failed: {other:?}")),
    };
    let cell = match body {
        Body::Cell(cell) => cell,
        Body::Cells(cells) if cells.len() == 1 => cells[0],
        other => return Err(format!("{start:?} returned {other:?}")),
    };
    let t1 = Instant::now();
    let watch = vm.submit(&Request::Output(cell))?;
    let mut first = None;
    loop {
        match vm.next(Duration::from_secs(60)) {
            Some(Event::Frame(f)) if f.id == watch => match Message::decode(&f) {
                Ok(Message::Output { .. }) => {
                    first.get_or_insert_with(Instant::now);
                }
                Ok(Message::Ended { state, .. }) => match first {
                    Some(_) if state == State::Exited(0) => break,
                    _ => return Err(format!("cell {cell} ended: {state:?}")),
                },
                Ok(Message::Done { .. }) => {}
                other => return Err(format!("watching cell {cell}: {other:?}")),
            },
            Some(Event::Text(t)) => vm.console.extend(t),
            Some(Event::Exited) | None => {
                return Err(format!(
                    "cell {cell} printed nothing; console:\n{}",
                    String::from_utf8_lossy(&vm.console)
                ));
            }
            Some(_) => {}
        }
    }
    done(vm, &Request::Kill(cell))?; // it ended: forget it
    let first = first.expect("output came before the end");
    Ok(FirstOutput {
        total: first - t0,
        reply,
        kernel: Duration::from_nanos(ns),
        watch: first - t1,
    })
}

/// Spawn and fork to first output (spec `benchmarks`, change
/// `measure-control-path`): `hello` run from its executable, and a
/// `sleeper` forked from a template frozen in its sleep (the child wakes
/// at once, prints `woken` and exits), each with where its time went.
fn first_outputs(vm: &mut Vm) -> Result<Vec<(&'static str, Vec<f64>)>> {
    let us = |d: Duration| d.as_secs_f64() * 1e6;
    let hello = Request::Run(Run {
        image: b"hello".to_vec(),
        argv: vec![b"hello".to_vec()],
        ..Run::default()
    });
    let (Body::Cell(template), _) = done(
        vm,
        &Request::Run(Run {
            image: b"sleeper".to_vec(),
            argv: ["sleeper", "3600000", "yield"]
                .iter()
                .map(|a| a.as_bytes().to_vec())
                .collect(),
            ..Run::default()
        }),
    )?
    else {
        return Err("`run` returned no cell".into());
    };
    let watch = vm.submit(&Request::Output(template))?;
    loop {
        match vm.next(Duration::from_secs(60)) {
            Some(Event::Frame(f)) if f.id == watch => {
                if let Ok(Message::Output { .. }) = Message::decode(&f) {
                    break; // `ready`
                }
            }
            Some(Event::Text(t)) => vm.console.extend(t),
            Some(Event::Exited) | None => return Err("the sleeper printed nothing".into()),
            Some(_) => {}
        }
    }
    // With no request to serve the kernel runs it into its sleep: frozen
    // before, a child would sleep the hour.
    thread::sleep(Duration::from_millis(100));
    done(vm, &Request::Freeze(template))?;
    let fork = Request::Fork {
        cell: template,
        n: 1,
    };
    let names = [
        "control.spawn_to_first_output_us",
        "breakdown.control.spawn_reply_us",
        "breakdown.control.spawn_kernel_us",
        "breakdown.control.spawn_watch_us",
        "control.fork_to_first_output_us",
        "breakdown.control.fork_reply_us",
        "breakdown.control.fork_kernel_us",
        "breakdown.control.fork_watch_us",
    ];
    let mut samples: Vec<(&str, Vec<f64>)> = names.iter().map(|&n| (n, Vec::new())).collect();
    for _ in 0..FIRST_OUTPUT_ROUNDS {
        for (i, start) in [&hello, &fork].into_iter().enumerate() {
            let f = first_output(vm, start)?;
            for (j, d) in [f.total, f.reply, f.kernel, f.watch]
                .into_iter()
                .enumerate()
            {
                samples[4 * i + j].1.push(us(d));
            }
        }
    }
    done(vm, &Request::Kill(template))?;
    done(vm, &Request::Kill(template))?;
    Ok(samples)
}

/// Bytes of the frames that time the serial line per byte.
const PROBE_BYTES: usize = 1024;
/// Quiet before an idle round trip: longer than the kernel keeps looking
/// at the serial line often after a request.
const QUIET: Duration = Duration::from_millis(300);
/// Idle round trips per boot.
const IDLE_ROUND_TRIPS: usize = 10;

/// Where a round trip's time goes (change `measure-control-path`): the
/// kernel's time for a `list`; what each byte costs each way, from
/// requests the kernel refuses at once, against a `list`: a `write-file` of
/// [`PROBE_BYTES`] to no cell (bytes in), and a `run` of an executable
/// whose [`PROBE_BYTES`]-byte name the refusal repeats (bytes in and out);
/// and a `list` after [`QUIET`], when the kernel looks less often.
fn serial_probes(vm: &mut Vm) -> Result<Vec<(&'static str, Vec<f64>)>> {
    let in_only = Request::WriteFile {
        cell: u32::MAX,
        path: b"/probe".to_vec(),
        data: vec![0; PROBE_BYTES],
    };
    let both_ways = Request::Run(Run {
        image: vec![b'x'; PROBE_BYTES],
        ..Run::default()
    });
    let mut timed = |request: &Request, quiet: Duration| -> Result<(f64, f64)> {
        let rounds = if quiet.is_zero() {
            OP_ROUNDS
        } else {
            IDLE_ROUND_TRIPS
        };
        let (mut took, mut kernel) = (Vec::new(), Vec::new());
        for _ in 0..rounds {
            thread::sleep(quiet);
            let (reply, d) = vm.call(request)?;
            let ns = match reply {
                Message::Done { ns, .. } | Message::Failed { ns, .. } => ns,
                other => return Err(format!("{request:?}: {other:?}")),
            };
            took.push(d.as_secs_f64() * 1e6);
            kernel.push(ns as f64 / 1e3);
        }
        Ok((summarize(&mut took).0, summarize(&mut kernel).0))
    };
    let (list, list_kernel) = timed(&Request::List, Duration::ZERO)?;
    let (into, _) = timed(&in_only, Duration::ZERO)?;
    let (both, _) = timed(&both_ways, Duration::ZERO)?;
    let (idle, _) = timed(&Request::List, QUIET)?;
    let per_byte = |us: f64| us.max(0.0) * 1e3 / PROBE_BYTES as f64;
    Ok(vec![
        ("breakdown.control.roundtrip_kernel_us", vec![list_kernel]),
        (
            "breakdown.control.serial_in_ns_per_byte",
            vec![per_byte(into - list)],
        ),
        (
            "breakdown.control.serial_out_ns_per_byte",
            vec![per_byte(both - into)],
        ),
        ("breakdown.control.idle_roundtrip_us", vec![idle]),
    ])
}

/// The host's fake model as the guest on `vmm` reaches it.
fn fake_model(vmm: Vmm) -> Result<String> {
    let bind = match vmm {
        Vmm::QemuVirt | Vmm::QemuMicrovm | Vmm::QemuQ35 => "127.0.0.1",
        other => crate::hostnet::host_address(other),
    };
    let port = crate::hostnet::llm_server(bind, 0)?;
    Ok(format!(
        "http://{}:{port}",
        crate::hostnet::host_address(vmm)
    ))
}

/// Starts the chat agent against the model at `url` and watches it.
fn chat_agent(vm: &mut Vm, url: &str) -> Result<u32> {
    let (Body::Cell(cell), _) = done(
        vm,
        &Request::Run(Run {
            image: b"cpython".to_vec(),
            files_from: b"cpython-files".to_vec(),
            argv: ["/usr/bin/python3", "/app/agent_chat.py", url, "fake"]
                .iter()
                .map(|a| a.as_bytes().to_vec())
                .collect(),
            input: true,
            ..Run::default()
        }),
    )?
    else {
        return Err("`run` returned no cell".into());
    };
    done(vm, &Request::Output(cell))?;
    Ok(cell)
}

/// Sends chat cell `cell` a message and waits for its turn to end.
fn chat_turn(vm: &mut Vm, cell: u32, partial: &mut Vec<u8>, turn: usize) -> Result<ChatTurn> {
    done(
        vm,
        &Request::Input {
            cell,
            data: format!("Turn {turn}: what is the sum?\n").into_bytes(),
        },
    )?;
    loop {
        if let Some(turn) = ChatTurn::parse(&chat_line(vm, cell, partial)?) {
            return Ok(turn);
        }
    }
}

/// How long the host's end of a snapshot transfer waits for the guest.
const TRANSFER_WAIT: Duration = Duration::from_secs(60);

/// How long a connection to the host's end of a transfer may take to send
/// its token.
const TOKEN_WAIT: Duration = Duration::from_secs(2);

/// The host's end of a snapshot transfer (design D2 of `snapshots-on-disk`):
/// a TCP listener the guest on a VMM reaches through its network card. It
/// takes only a connection from the guest's address that first sends the
/// request's token: another local process can neither read a file being
/// loaded nor pass bytes off as a saved cell.
#[derive(Debug)]
pub struct Transfer {
    listener: std::net::TcpListener,
    /// Where the guest's connections come from, as the host sees them.
    guest: std::net::IpAddr,
    /// Where the guest reaches it, and the token.
    pub host: rhizo_control::Host,
}

/// A random token, from the host's generator.
fn token() -> Result<u64> {
    let mut b = [0u8; 8];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .map_err(|e| format!("no random token: {e}"))?;
    Ok(u64::from_le_bytes(b))
}

impl Transfer {
    /// A listener for the guest on `vmm`, where it reaches the host.
    pub fn new(vmm: Vmm) -> Result<Self> {
        let guest_sees = crate::hostnet::host_address(vmm);
        let bind = match vmm {
            Vmm::QemuVirt | Vmm::QemuMicrovm | Vmm::QemuQ35 => "127.0.0.1",
            _ => guest_sees,
        };
        let listener = std::net::TcpListener::bind((bind, 0))
            .and_then(|l| l.set_nonblocking(true).map(|()| l))
            .map_err(|e| format!("cannot listen on {bind} for the transfer: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let ip: std::net::Ipv4Addr = guest_sees.parse().map_err(|e| format!("{e}"))?;
        let guest = crate::hostnet::guest_address(vmm)
            .parse()
            .map_err(|e| format!("{e}"))?;
        Ok(Self {
            listener,
            guest,
            host: rhizo_control::Host {
                ip: ip.octets(),
                port,
                token: token()?,
            },
        })
    }

    /// The guest's connection, within [`TRANSFER_WAIT`]: from its address
    /// and with the token first; any other is closed unanswered.
    fn accept(&self) -> Result<std::net::TcpStream> {
        let deadline = Instant::now() + TRANSFER_WAIT;
        loop {
            match self.listener.accept() {
                Ok((mut stream, peer)) => {
                    let mut got = [0u8; 8];
                    let admitted = peer.ip() == self.guest
                        && stream.set_nonblocking(false).is_ok()
                        && stream.set_read_timeout(Some(TOKEN_WAIT)).is_ok()
                        && stream.read_exact(&mut got).is_ok()
                        && u64::from_le_bytes(got) == self.host.token
                        && stream.set_read_timeout(None).is_ok();
                    if admitted {
                        return Ok(stream);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() > deadline {
                        return Err("the guest did not connect for the transfer".into());
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                Err(e) => return Err(format!("transfer: {e}")),
            }
        }
    }

    /// Receives, in a thread, everything the guest sends until it closes.
    pub fn receive(self) -> thread::JoinHandle<Result<Vec<u8>>> {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            self.accept()?
                .read_to_end(&mut bytes)
                .map_err(|e| format!("transfer: {e}"))?;
            Ok(bytes)
        })
    }

    /// Sends `bytes` to the guest, in a thread.
    pub fn send(self, bytes: Vec<u8>) -> thread::JoinHandle<Result> {
        thread::spawn(move || {
            let mut stream = self.accept()?;
            stream
                .write_all(&bytes)
                .and_then(|()| stream.shutdown(std::net::Shutdown::Write))
                .map_err(|e| format!("transfer: {e}"))
        })
    }
}

/// The snapshot of suspended cell or template `cell`, and the round trip
/// that brought it.
fn save(vm: &mut Vm, vmm: Vmm, cell: u32) -> Result<(Vec<u8>, Duration)> {
    let transfer = Transfer::new(vmm)?;
    let host = transfer.host;
    let received = transfer.receive();
    let t0 = Instant::now();
    done(vm, &Request::Save { cell, host })?;
    let took = t0.elapsed();
    let bytes = received.join().map_err(|_| "the transfer panicked")??;
    Ok((bytes, took))
}

/// Loads `bytes` as a suspended cell or a template; the cell, and the round
/// trip.
fn load(vm: &mut Vm, vmm: Vmm, bytes: Vec<u8>, template: bool) -> Result<(u32, Duration)> {
    let transfer = Transfer::new(vmm)?;
    let (host, size) = (transfer.host, bytes.len() as u64);
    let sent = transfer.send(bytes);
    let (Body::Cell(cell), took) = done(
        vm,
        &Request::Load {
            size,
            template,
            host,
        },
    )?
    else {
        return Err("`load` returned no cell".into());
    };
    sent.join().map_err(|_| "the transfer panicked")??;
    Ok((cell, took))
}

/// Snapshot files (spec `benchmarks`, design D6 of `snapshots-on-disk`):
/// the chat agent frozen into a template after one turn, saved, and loaded
/// into a freshly booted VM, where a cell of it answers; then a cold start
/// on that VM.
fn snapshot_files(
    mut vm: Vm,
    vmm: Vmm,
    kernel: &Path,
    opts: RunOptions,
) -> Result<Vec<(&'static str, &'static str, f64)>> {
    let us = |d: Duration| d.as_secs_f64() * 1e6;
    let url = fake_model(vmm)?;
    let agent = chat_agent(&mut vm, &url)?;
    let mut partial = Vec::new();
    while chat_line(&mut vm, agent, &mut partial)? != CHAT_READY {}
    chat_turn(&mut vm, agent, &mut partial, 0)?;
    done(&mut vm, &Request::Freeze(agent))?;
    let (file, saved) = save(&mut vm, vmm, agent)?;
    vm.shutdown();
    let mut vm = Vm::start(vmm, kernel, opts, Stdio::null())?;
    let size = file.len();
    let (template, loaded) = load(&mut vm, vmm, file, true)?;
    let t0 = Instant::now();
    let (Body::Cells(cells), _) = done(
        &mut vm,
        &Request::Fork {
            cell: template,
            n: 1,
        },
    )?
    else {
        return Err("`fork` returned no cells".into());
    };
    done(&mut vm, &Request::Output(cells[0]))?;
    let mut partial = Vec::new();
    chat_turn(&mut vm, cells[0], &mut partial, 1)?;
    let loaded_answer = t0.elapsed();
    let t0 = Instant::now();
    let cold = chat_agent(&mut vm, &url)?;
    let mut partial = Vec::new();
    while chat_line(&mut vm, cold, &mut partial)? != CHAT_READY {}
    chat_turn(&mut vm, cold, &mut partial, 0)?;
    let cold_answer = t0.elapsed();
    vm.shutdown();
    Ok(vec![
        ("snapshot.save_us", "us", us(saved)),
        ("snapshot.load_us", "us", us(loaded)),
        ("snapshot.file_bytes", "bytes", size as f64),
        ("snapshot.loaded_answer_us", "us", us(loaded_answer)),
        ("snapshot.cold_answer_us", "us", us(cold_answer)),
    ])
}

/// Turns timed per boot for `chat.turn_us`.
const CHAT_TURNS: usize = 20;

/// The Rhizo side of interactive turns (spec `benchmarks`, design D6 of
/// `add-interactive-chat`): a chat agent against the host's fake model,
/// each turn timed from the host's `input` request to the agent's last line
/// read back, less the time the agent waited for the model.
fn chat_turns(vm: &mut Vm, vmm: Vmm) -> Result<Vec<f64>> {
    let cell = chat_agent(vm, &fake_model(vmm)?)?;
    let mut partial = Vec::new();
    while chat_line(vm, cell, &mut partial)? != CHAT_READY {}
    let mut samples = Vec::new();
    for turn in 0..CHAT_TURNS {
        let t0 = Instant::now();
        let turn = chat_turn(vm, cell, &mut partial, turn)?;
        let total = t0.elapsed().as_nanos() as u64;
        samples.push(total.saturating_sub(turn.model_ns) as f64 / 1e3);
    }
    done(vm, &Request::Kill(cell))?;
    done(vm, &Request::Kill(cell))?;
    Ok(samples)
}

/// The next line watched cell `cell` prints.
fn chat_line(vm: &mut Vm, cell: u32, partial: &mut Vec<u8>) -> Result<String> {
    loop {
        if let Some(end) = partial.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = partial.drain(..=end).collect();
            return Ok(String::from_utf8_lossy(&line[..end]).into_owned());
        }
        match vm.next(CHAT_STEP) {
            Some(Event::Frame(f)) => match Message::decode(&f) {
                Ok(Message::Output { cell: c, data }) if c == cell => partial.extend(data),
                Ok(Message::Ended { cell: c, state }) if c == cell => {
                    return Err(format!("the chat agent ended: {state:?}"));
                }
                _ => {}
            },
            Some(Event::Text(t)) => vm.console.extend(t),
            Some(Event::Exited) | None => {
                return Err(format!(
                    "the chat agent printed nothing; console:\n{}",
                    String::from_utf8_lossy(&vm.console)
                ));
            }
            Some(Event::Bad { .. }) => {}
        }
    }
}

mod expected {
    include!("../../cells/expected.rs");
}

/// How long a chat step may take (CPython under emulation starts in
/// seconds).
const CHAT_STEP: Duration = Duration::from_secs(120);

/// `rhizo chat` driven through pipes, as someone typing would; every line
/// it prints goes to `log` as `chat| LINE`.
struct Chat<'a> {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    log: &'a std::cell::RefCell<String>,
}

impl<'a> Chat<'a> {
    fn start(
        rhizo: &Path,
        state: &Path,
        words: &[&str],
        log: &'a std::cell::RefCell<String>,
    ) -> Result<Self> {
        log.borrow_mut()
            .push_str(&format!("$ rhizo {}\n", words.join(" ")));
        let mut child = Command::new(rhizo)
            .arg("--state")
            .arg(state)
            .args(words)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run {}: {e}", rhizo.display()))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout is piped");
        let (tx, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in std::io::BufRead::lines(std::io::BufReader::new(stdout))
                .map_while(std::result::Result::ok)
            {
                let _ = tx.send(line);
            }
        });
        Ok(Chat {
            child,
            stdin,
            lines,
            log,
        })
    }

    /// The lines printed up to the first that starts with `end`, that one
    /// included.
    fn until(&mut self, end: &str) -> Result<String> {
        let mut seen = String::new();
        loop {
            let line = self
                .lines
                .recv_timeout(CHAT_STEP)
                .map_err(|_| format!("`rhizo chat` printed no `{end}`"))?;
            self.log.borrow_mut().push_str(&format!("chat| {line}\n"));
            seen.push_str(&line);
            seen.push('\n');
            if line.starts_with("[rhizo] error") {
                return Err(format!("`rhizo chat` said: {line}"));
            }
            if line.starts_with(end) {
                return Ok(seen);
            }
        }
    }

    /// Types `line`, then reads up to `end`.
    fn send(&mut self, line: &str, end: &str) -> Result<String> {
        let stdin = self.stdin.as_mut().ok_or("the chat's input is closed")?;
        stdin
            .write_all(format!("{line}\n").as_bytes())
            .map_err(|e| format!("cannot type into `rhizo chat`: {e}"))?;
        self.until(end)
    }

    /// Closes its input (Ctrl-D) and waits for it to end: whether it
    /// succeeded, and what it printed last.
    fn finish(mut self) -> Result<(bool, String)> {
        drop(self.stdin.take());
        let deadline = Instant::now() + CHAT_STEP;
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                _ => return Err("`rhizo chat` did not end after its input closed".into()),
            }
        };
        let rest: String = self.lines.iter().map(|l| format!("{l}\n")).collect();
        for line in rest.lines() {
            self.log.borrow_mut().push_str(&format!("chat| {line}\n"));
        }
        Ok((status.success(), rest))
    }
}

impl Drop for Chat<'_> {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Builds the `rhizo` tool (debug) and returns its path.
pub fn build_rhizo() -> Result<PathBuf> {
    crate::cargo(&["build", "-p", "rhizo-cli", "--bin", "rhizo"])?;
    Ok(target_dir().join("debug").join("rhizo"))
}

/// The `control` test step (spec `control-plane`): boots `kernel` with
/// `rhizo up` on `vmm` and drives it with `rhizo` as a user would: a
/// program run to its end, an idle cell suspended, resumed, forked, killed
/// and listed, and the CPython agent against the host's fake model, its
/// transcript checked.
pub fn check(vmm: Vmm, accel: Accel, kernel: &Path, rhizo: &Path, reproduce: &str) -> Result {
    let state = target_dir()
        .join("xtask-test")
        .join(arch::current().id())
        .join(format!("control-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state)
        .map_err(|e| format!("cannot create {}: {e}", state.display()))?;
    let log = std::cell::RefCell::new(String::new());
    let rhizo_run = |words: &[&str]| -> Result<(bool, String)> {
        let out = Command::new(rhizo)
            .arg("--state")
            .arg(&state)
            .args(words)
            .current_dir(workspace_root())
            .output()
            .map_err(|e| format!("cannot run {}: {e}", rhizo.display()))?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr);
        log.borrow_mut()
            .push_str(&format!("$ rhizo {}\n{stdout}{stderr}", words.join(" ")));
        Ok((out.status.success(), format!("{stdout}{stderr}")))
    };
    let fail = |what: &str, expected: &str| {
        let console = std::fs::read_to_string(state.join("console.log")).unwrap_or_default();
        Failure::new("control", what)
            .expected(expected)
            .actual({
                // What rhizo said last first: a summary of the failure shows
                // only its first line.
                let log = log.borrow();
                let last = log
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("");
                format!("{last}\nrhizo said:\n{log}\nconsole:\n{console}")
            })
            .hint("the commands and their output are above, then the kernel's console")
            .reproduce(reproduce)
            .into_error()
    };
    let ok = |words: &[&str]| -> Result<String> {
        match rhizo_run(words)? {
            (true, out) => Ok(out),
            (false, _) => Err(fail(
                &format!("`rhizo {}` failed", words.join(" ")),
                "success",
            )),
        }
    };
    let must = |cond: bool, what: &str, expected: &str| -> Result {
        if cond {
            Ok(())
        } else {
            Err(fail(what, expected))
        }
    };
    let kernel_path = kernel.to_string_lossy().into_owned();
    let up = || {
        ok(&[
            "up",
            "--vmm",
            vmm.id(),
            "--arch",
            arch::current().id(),
            "--accel",
            accel.id(),
            "--kernel",
            &kernel_path,
        ])
    };
    up()?;
    // Saved snapshots live beside the state, which `down` empties.
    let files = state.with_extension("files");
    let _ = std::fs::remove_dir_all(&files);
    std::fs::create_dir_all(&files)
        .map_err(|e| format!("cannot create {}: {e}", files.display()))?;
    let scenario = || -> Result {
        // A program the kernel holds, shown to its end.
        let out = ok(&["run", "hello"])?;
        let hello = expected::PROGRAMS
            .iter()
            .find(|p| p.0 == "hello")
            .map_or("", |p| p.1);
        must(
            out.contains(hello) && out.contains(" exited 0"),
            "`rhizo run hello` shows the cell's output and its exit",
            &format!("{hello:?} and `exited 0`"),
        )?;
        // An idle cell through every operation.
        let out = ok(&["run", "idle", "--detach"])?;
        let cell = out
            .split("cell ")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .filter(|id| id.parse::<u32>().is_ok())
            .ok_or_else(|| fail("`rhizo run --detach` names the cell", "`cell N started`"))?
            .to_string();
        let line_of = |out: &str, id: &str, state: &str| {
            out.lines()
                .any(|l| l.split_whitespace().next() == Some(id) && l.contains(state))
        };
        let out = ok(&["suspend", &cell])?;
        must(
            out.contains(&format!("suspended {cell} (")) && out.contains("round trip"),
            "`rhizo suspend` reports the snapshot and its times",
            "`suspended N (... KiB snapshot) in ... (round trip ...)`",
        )?;
        let out = ok(&["ps"])?;
        must(
            line_of(&out, &cell, "suspended"),
            "`rhizo ps` lists the suspended cell",
            "a line `N suspended (...) idle`",
        )?;
        ok(&["resume", &cell])?;
        let out = ok(&["fork", &cell, "2"])?;
        let children: Vec<String> = out
            .split(['[', ']'])
            .nth(1)
            .unwrap_or_default()
            .split(", ")
            .filter(|c| !c.is_empty())
            .map(String::from)
            .collect();
        must(
            children.len() == 2,
            "`rhizo fork N 2` makes two cells",
            "`forked N into [A, B]`",
        )?;
        // A resume takes no identifier: the children are the next two.
        let n: u32 = cell.parse().unwrap_or_default();
        must(
            children == [(n + 1).to_string(), (n + 2).to_string()],
            "a fork's cells take the next identifiers, after a suspend and resume",
            &format!("`forked {n} into [{}, {}]`", n + 1, n + 2),
        )?;
        for c in children.iter().chain([&cell]) {
            ok(&["kill", c])?;
        }
        let out = ok(&["ps"])?;
        must(
            children
                .iter()
                .chain([&cell])
                .all(|c| line_of(&out, c, "killed")),
            "`rhizo ps` lists the killed cells",
            "a `killed` line for the cell and both children",
        )?;
        ok(&["kill", &cell])?;
        let out = ok(&["ps"])?;
        must(
            !line_of(&out, &cell, ""),
            "killing an ended cell forgets it",
            "no line for it in `rhizo ps`",
        )?;
        let refused = |words: &[&str]| -> Result {
            let (succeeded, _) = rhizo_run(words)?;
            must(
                !succeeded,
                &format!("`rhizo {}` is refused", words.join(" ")),
                "a failure",
            )
        };
        refused(&["suspend", "99999"])?;
        refused(&["run", "no-such-program"])?;
        let out = ok(&["run", "idle", "--detach"])?;
        let idle = out
            .split("cell ")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .unwrap_or_default()
            .to_string();
        refused(&["resume", &idle])?;
        ok(&["kill", &idle])?;
        // The agent, against the host's fake model.
        if crate::cpython::root().join("usr/bin/python3").exists() {
            let bind = match vmm {
                Vmm::QemuVirt | Vmm::QemuMicrovm | Vmm::QemuQ35 => "127.0.0.1",
                other => crate::hostnet::host_address(other),
            };
            let port = crate::hostnet::llm_server(bind, 0)?;
            let llm = format!("http://{bind}:{port}");
            let out = ok(&[
                "run",
                "python",
                "--file",
                "/app/answer.py=import os; print(6 * 7, os.environ['WHO'])",
                "--env",
                "WHO=cell",
                "--",
                "/app/answer.py",
            ])?;
            must(
                out.contains("42 cell") && out.contains(" exited 0"),
                "`rhizo run python` runs a file it was given, with its environment",
                "`42 cell` and `exited 0`",
            )?;
            let turns = expected::AGENT_TURNS.to_string();
            let out = ok(&[
                "run",
                "python-agent",
                "--llm",
                &llm,
                "--model",
                "fake",
                "--turns",
                &turns,
                "--transcript",
            ])?;
            let transcript: String = out
                .lines()
                .filter(|l| !l.starts_with("[rhizo]") && !l.starts_with("rhizo-demo"))
                .map(|l| format!("{l}\n"))
                .collect();
            must(
                transcript == expected::AGENT_TRANSCRIPT,
                "the agent through the control plane prints the recorded transcript",
                expected::AGENT_TRANSCRIPT,
            )?;
            must(
                out.matches("[rhizo] suspended between turns").count() == expected::AGENT_TURNS
                    && out.contains("[rhizo] forked the agent")
                    && out.contains("rhizo-demo done"),
                "the agent is suspended after every turn and forked at its decision point",
                "one `suspended between turns` per turn, `forked the agent`, `rhizo-demo done`",
            )?;
            // A conversation through `rhizo chat`, typed through a pipe.
            let chat = |words: &[&str]| {
                Chat::start(rhizo, &state, words, &log).map_err(|e| fail(&e, "a chat"))
            };
            let step = |chat: &mut Chat, line: &str, end: &str| {
                chat.send(line, end).map_err(|e| fail(&e, end))
            };
            let mut first = chat(&["chat", "--llm", &llm, "--model", "fake"])?;
            let started = first
                .until("[rhizo] ready")
                .map_err(|e| fail(&e, "ready"))?;
            let parent = started
                .split_once("[rhizo] chat cell ")
                .and_then(|(_, rest)| rest.split_once(':'))
                .map(|(id, _)| id.to_string())
                .ok_or_else(|| fail("`rhizo chat` names its cell", "`chat cell N:`"))?;
            for message in &expected::CHAT_MESSAGES[..2] {
                step(&mut first, message, "[rhizo] model ")?;
            }
            let forked = step(&mut first, "/fork 2", "[rhizo] forked")?;
            let children: Vec<String> = forked
                .split_once("into [")
                .and_then(|(_, rest)| rest.split_once(']'))
                .map(|(ids, _)| ids.split(", ").map(String::from).collect())
                .unwrap_or_default();
            must(
                children.len() == 2,
                "`/fork 2` makes two branches",
                "`forked cell N into [A, B]`",
            )?;
            let listed = step(&mut first, "/branches", "[rhizo] 3 branches")?;
            must(
                listed.matches(" running").count() == 3,
                "`/branches` lists the conversation and its two branches, running",
                "three `running` branches",
            )?;
            let branch = &children[0];
            step(
                &mut first,
                &format!("/switch {branch}"),
                "[rhizo] now talking",
            )?;
            step(&mut first, expected::CHAT_MESSAGES[2], "[rhizo] model ")?;
            step(&mut first, "/suspend", "[rhizo] suspended cell")?;
            must(
                first.finish().map_err(|e| fail(&e, "an exit"))?.0,
                "`/suspend` leaves the chat",
                "`rhizo chat` exits with 0",
            )?;
            let mut again = chat(&["chat", branch])?;
            let resumed = again
                .until("[rhizo] talking to cell")
                .map_err(|e| fail(&e, "talking"))?;
            must(
                resumed.contains("resumed in"),
                "`rhizo chat N` resumes a suspended branch",
                "`talking to cell N, resumed in ...`",
            )?;
            step(&mut again, expected::CHAT_MESSAGES[3], "[rhizo] model ")?;
            // Two lines typed at once: the second waits for the first's
            // turn, and the chat says so (change `fix-chat-tools`).
            let both = expected::CHAT_MESSAGES[4..].join("\n");
            let first_turn = step(&mut again, &both, "[rhizo] model ")?;
            let second_turn = again
                .until("[rhizo] model ")
                .map_err(|e| fail(&e, "[rhizo] model "))?;
            let typed = first_turn + &second_turn;
            must(
                typed.matches("[rhizo] queued for the next turn").count() == 1,
                "a line typed while a turn runs is queued, and the chat says so once",
                "one `[rhizo] queued for the next turn`",
            )?;
            must(
                typed.contains("  [tool] ->\n      12.0\n"),
                "a snippet ending in an expression returns its value to the model",
                "the tool result `12.0`",
            )?;
            let (exited, rest) = again.finish().map_err(|e| fail(&e, "an exit"))?;
            must(
                exited && rest.contains(&format!("[rhizo] cell {branch} exited 0")),
                "the end of the chat's input ends the agent",
                "`cell N exited 0`",
            )?;
            let transcript: String = log
                .borrow()
                .lines()
                .filter(|l| l.starts_with("chat| "))
                .map(|l| &l[6..])
                .filter(|l| !l.starts_with("[rhizo]") && !l.starts_with("> /"))
                .map(|l| format!("{l}\n"))
                .collect();
            must(
                transcript == expected::CHAT_TRANSCRIPT,
                "the chat prints the recorded transcript",
                expected::CHAT_TRANSCRIPT,
            )?;
            must(
                log.borrow().matches("chat| [rhizo] model ").count()
                    == expected::CHAT_MESSAGES.len(),
                "every turn prints the model's time apart from Rhizo's",
                "one `[rhizo] model ..., rhizo ...` line per message",
            )?;
            for other in children[1..].iter().chain([&parent]) {
                ok(&["kill", other])?;
            }
            // Snapshots as files (spec `snapshots`, change
            // `snapshots-on-disk`): a conversation frozen into a template
            // and another suspended, each after one turn, saved, then
            // loaded into a fresh VM, where two cells of the template and
            // the resumed cell each answer a turn right.
            let template_file = files.join("agent-template.rhz");
            let cell_file = files.join("agent-cell.rhz");
            let path = |p: &Path| p.to_string_lossy().into_owned();
            let one_turn = || -> Result<String> {
                let mut c = chat(&["chat", "--llm", &llm, "--model", "fake"])?;
                let started = c.until("[rhizo] ready").map_err(|e| fail(&e, "ready"))?;
                let id = started
                    .split_once("[rhizo] chat cell ")
                    .and_then(|(_, rest)| rest.split_once(':'))
                    .map(|(id, _)| id.to_string())
                    .ok_or_else(|| fail("`rhizo chat` names its cell", "`chat cell N:`"))?;
                let turn = step(&mut c, expected::CHAT_MESSAGES[0], "[rhizo] model ")?;
                must(
                    answers_right(&turn),
                    "the agent answers before it is saved",
                    "the sum its snippet printed",
                )?;
                step(&mut c, "/suspend", "[rhizo] suspended cell")?;
                c.finish().map_err(|e| fail(&e, "an exit"))?;
                Ok(id)
            };
            let template = one_turn()?;
            ok(&["resume", &template])?;
            ok(&["freeze", &template])?;
            let out = ok(&["ps"])?;
            must(
                line_of(&out, &template, "template"),
                "`rhizo ps` lists the frozen cell as a template",
                "a line `N template ...`",
            )?;
            let out = ok(&["save", &template, &path(&template_file)])?;
            must(
                out.contains(&format!("saved {template} to ")) && out.contains("MiB/s"),
                "`rhizo save` reports the file, its size and its rate",
                "`saved N to FILE (... MiB) in ... (round trip ..., ... MiB/s)`",
            )?;
            let suspended = one_turn()?;
            ok(&["save", &suspended, &path(&cell_file)])?;
            refused(&["save", "99999", &path(&cell_file)])?;
            ok(&["down"])?;
            up()?;
            let out = ok(&["load", &path(&template_file), "--template"])?;
            let loaded = out
                .split_once(" as template ")
                .and_then(|(_, rest)| rest.split(' ').next())
                .map(String::from)
                .ok_or_else(|| {
                    fail(
                        "`rhizo load --template` names the template",
                        "`as template N`",
                    )
                })?;
            let out = ok(&["fork", &loaded, "2"])?;
            let copies: Vec<String> = out
                .split(['[', ']'])
                .nth(1)
                .unwrap_or_default()
                .split(", ")
                .filter(|c| !c.is_empty())
                .map(String::from)
                .collect();
            must(
                copies.len() == 2,
                "`rhizo fork` makes two cells of a loaded template",
                "`forked N into [A, B]`",
            )?;
            let answer = |id: &str, then: &str| -> Result {
                let mut c = chat(&["chat", id])?;
                let said = c
                    .until("[rhizo] talking to cell")
                    .map_err(|e| fail(&e, "talking"))?;
                must(
                    said.contains(then),
                    &format!("`rhizo chat {id}` talks to the loaded cell"),
                    then,
                )?;
                let turn = step(&mut c, expected::CHAT_MESSAGES[1], "[rhizo] model ")?;
                must(
                    answers_right(&turn),
                    &format!("cell {id}, loaded after a reboot, answers right"),
                    "the sum its snippet printed",
                )?;
                c.finish().map_err(|e| fail(&e, "an exit"))?;
                Ok(())
            };
            for copy in &copies {
                answer(copy, "")?;
            }
            let out = ok(&["load", &path(&cell_file)])?;
            let resumed = out
                .split_once(" as suspended cell ")
                .and_then(|(_, rest)| rest.split(' ').next())
                .map(String::from)
                .ok_or_else(|| fail("`rhizo load` names the cell", "`as suspended cell N`"))?;
            answer(&resumed, "resumed in")?;
            // Damaged, cut and foreign files are refused, with the reason.
            let bytes = std::fs::read(&cell_file).map_err(|e| e.to_string())?;
            let mut damaged = bytes.clone();
            damaged[bytes.len() / 2] ^= 0x40;
            let mut foreign = bytes.clone();
            let other: u16 = match arch::current() {
                arch::Arch::X86_64 => 183,
                arch::Arch::Aarch64 => 62,
            };
            foreign[12..14].copy_from_slice(&other.to_le_bytes());
            for (name, contents, reason) in [
                ("damaged", damaged, "checksum mismatch"),
                (
                    "cut",
                    bytes[..bytes.len() / 2].to_vec(),
                    "checksum mismatch",
                ),
                ("foreign", foreign, "taken on "),
            ] {
                let file = files.join(format!("{name}.rhz"));
                std::fs::write(&file, contents).map_err(|e| e.to_string())?;
                let (loaded, said) = rhizo_run(&["load", &path(&file)])?;
                must(
                    !loaded && said.contains(reason),
                    &format!("a {name} file is refused with the reason"),
                    reason,
                )?;
            }
            ok(&["kill", &loaded])?;
        } else {
            report::text("[xtask] control: CPython not built, the agent part did not run");
        }
        let out = ok(&["stats"])?;
        must(
            out.contains("memory:") && out.contains("MiB free"),
            "`rhizo stats` reports memory",
            "`memory: ... MiB free of ... MiB`",
        )
    };
    let result = scenario();
    let down = ok(&["down"]);
    result.and(down.map(|_| ()))?;
    let mut left: Vec<String> = std::fs::read_dir(&state)
        .map_err(|e| format!("cannot read {}: {e}", state.display()))?
        .filter_map(|e| Some(e.ok()?.file_name().to_string_lossy().into_owned()))
        .collect();
    left.sort();
    must(
        left == ["console.log"],
        &format!("`rhizo down` leaves only the console log, not {left:?}"),
        "[\"console.log\"]",
    )?;
    let _ = std::fs::remove_dir_all(&state);
    let _ = std::fs::remove_dir_all(&files);
    Ok(())
}

/// Whether a turn of the chat agent against the fake model answers right:
/// the sum of the squares below the number its snippet uses.
fn answers_right(turn: &str) -> bool {
    let number = |after: &str, end: char| {
        turn.split_once(after)?
            .1
            .split(end)
            .next()?
            .parse::<u64>()
            .ok()
    };
    match (number("range(", ')'), number("The snippet printed ", '.')) {
        (Some(n), Some(sum)) => n > 0 && sum == (n - 1) * n * (2 * n - 1) / 6,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake VMM's vsock socket that answers `CONNECT 1024` with `answer`,
    /// then sends `hello` and returns what it reads back (4 bytes).
    fn fake_vmm(path: &Path, answer: &'static [u8]) -> thread::JoinHandle<Vec<u8>> {
        let _ = std::fs::remove_file(path);
        let listener = std::os::unix::net::UnixListener::bind(path).unwrap();
        thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut line = [0u8; 13];
            s.read_exact(&mut line).unwrap();
            assert_eq!(&line, b"CONNECT 1024\n");
            s.write_all(answer).unwrap();
            s.write_all(b"hello").unwrap();
            let mut back = vec![0u8; 4];
            let _ = s.read_exact(&mut back);
            back
        })
    }

    /// Firecracker's and Cloud Hypervisor's handshake (design D4 of
    /// `control-over-vsock`): the guest's bytes follow the `OK` line, and
    /// another answer is a refusal.
    #[test]
    fn the_vmm_s_socket_connects_to_the_guest_s_port() {
        let path = std::env::temp_dir().join(format!("rhizo-test-{}.sock", std::process::id()));
        let vmm = fake_vmm(&path, b"OK 1073741824\n");
        let (mut from, mut to) = connect_unix(&path).unwrap();
        let mut got = [0u8; 5];
        from.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"hello", "nothing of the guest's bytes is lost");
        to.write_all(b"ping").unwrap();
        assert_eq!(vmm.join().unwrap(), b"ping");
        let vmm = fake_vmm(&path, b"no such port\n");
        let refused = connect_unix(&path).err().map(|e| e.to_string());
        assert_eq!(
            refused.as_deref(),
            Some("the VMM answered \"no such port\"")
        );
        drop(vmm);
        let _ = std::fs::remove_file(&path);
    }

    /// The host's end of a transfer takes only the connection that sends
    /// the request's token first (design D2 of `snapshots-on-disk`): an
    /// impostor that connects first gets nothing of a file being loaded,
    /// and its bytes are not taken for a saved cell.
    #[test]
    fn transfers_take_only_the_token_s_connection() {
        use std::net::TcpStream;
        let connect = |t: &rhizo_control::Host, token: u64, data: &[u8]| {
            let mut s = TcpStream::connect(("127.0.0.1", t.port)).unwrap();
            s.write_all(&token.to_le_bytes()).unwrap();
            s.write_all(data).unwrap();
            s
        };
        // Save: the impostor's bytes are dropped, the guest's kept.
        let t = Transfer::new(Vmm::QemuMicrovm).unwrap();
        let host = t.host;
        let received = t.receive();
        let mut impostor = connect(&host, host.token ^ 1, b"not the cell");
        let mut rest = Vec::new();
        let _ = impostor.read_to_end(&mut rest); // closed, or reset
        assert!(rest.is_empty(), "closed unanswered");
        drop(connect(&host, host.token, b"the cell"));
        assert_eq!(received.join().unwrap().unwrap(), b"the cell");
        // Load: the impostor reads nothing, the guest the whole file.
        let t = Transfer::new(Vmm::QemuMicrovm).unwrap();
        let host = t.host;
        let sent = t.send(b"the file".to_vec());
        let mut impostor = connect(&host, host.token.wrapping_add(1), b"");
        let mut leaked = Vec::new();
        let _ = impostor.read_to_end(&mut leaked);
        assert!(leaked.is_empty(), "nothing for the impostor");
        let mut guest = connect(&host, host.token, b"");
        let mut file = Vec::new();
        guest.read_to_end(&mut file).unwrap();
        assert_eq!(file, b"the file");
        sent.join().unwrap().unwrap();
        // Tokens differ from one transfer to the next.
        assert_ne!(
            host.token,
            Transfer::new(Vmm::QemuMicrovm).unwrap().host.token
        );
    }

    #[test]
    fn answers_are_checked() {
        let turn = "  [tool] run_python\n      print(sum(i * i for i in range(641)))\n\
                    The snippet printed 87586240.\n";
        assert!(answers_right(turn));
        assert!(!answers_right(&turn.replace("87586240", "87586241")));
        assert!(!answers_right("The snippet printed 5."));
    }

    #[test]
    fn the_chat_agent_s_turn_lines_are_read() {
        assert_eq!(
            ChatTurn::parse("\x1eturn model_ns=12 agent_ns=345"),
            Some(ChatTurn {
                model_ns: 12,
                agent_ns: 345
            })
        );
        for other in [
            "turn model_ns=12 agent_ns=345",
            "\x1eturn model_ns=12",
            "\x1eturn model_ns=x agent_ns=1",
            "\x1eturn agent_ns=1 model_ns=2",
            "\x1eturn model_ns=1 agent_ns=2 more",
            CHAT_READY,
        ] {
            assert_eq!(ChatTurn::parse(other), None, "{other:?}");
        }
    }
}
