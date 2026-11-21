//! A control kernel on a VMM, driven over its serial line (spec
//! `control-plane`, design D2 of `add-control-plane`): the VMM's standard
//! input carries requests, its standard output the kernel's console text
//! and its replies. `rhizo up` keeps one of these behind a socket; the
//! benchmark times requests through one; the `control` test step drives
//! `rhizo` itself against one.

use crate::bench::{Metric, summarize};
use crate::report::Failure;
use crate::vmm::{self, Accel, Guest, RunOptions, Vmm};
use crate::{Profile, Result, arch, build_kernel, report, target_dir, workspace_root};
use rhizo_control::{Body, Decoder, Frame, Item, Message, Request, Run};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// How long a control kernel may take to boot and say hello.
pub const BOOT_TIMEOUT: Duration = Duration::from_secs(120);

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

/// Sends frames to a VM, from any thread.
#[derive(Debug, Clone)]
pub struct Writer(Arc<Mutex<ChildStdin>>);

impl Writer {
    /// Sends bytes (whole frames) to the kernel.
    pub fn send(&self, bytes: &[u8]) -> Result {
        let mut stdin = self.0.lock().unwrap_or_else(|e| e.into_inner());
        stdin
            .write_all(bytes)
            .and_then(|()| stdin.flush())
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
        control: true,
    }
}

impl Vm {
    /// Boots `kernel` (a `control` build) on `vmm` and waits for its hello.
    /// The VMM's error output goes to `stderr`.
    pub fn start(vmm: Vmm, kernel: &Path, opts: RunOptions, stderr: Stdio) -> Result<Self> {
        if let Some(reason) = vmm.unavailable_reason(opts.accel) {
            return Err(format!("{} is not available: {reason}", vmm.id()));
        }
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
        let mut stdout = child.stdout.take().expect("stdout is piped");
        let (tx, events) = mpsc::channel();
        thread::spawn(move || {
            let mut decoder = Decoder::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = stdout.read(&mut buf) {
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
            let _ = tx.send(Event::Exited);
        });
        let mut vm = Vm {
            child,
            stdin: Writer(Arc::new(Mutex::new(stdin))),
            events,
            console: Vec::new(),
            next_id: 1,
        };
        let deadline = Instant::now() + BOOT_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match vm.next(left) {
                Some(Event::Frame(f))
                    if matches!(Message::decode(&f), Ok(Message::Hello { .. })) =>
                {
                    return Ok(vm);
                }
                Some(Event::Text(t)) => vm.console.extend(t),
                Some(Event::Exited) | None => {
                    let _ = vm.child.kill();
                    let _ = vm.child.wait();
                    return Err(format!(
                        "the control kernel on {} did not say hello; its console:\n{}",
                        vmm.id(),
                        String::from_utf8_lossy(&vm.console)
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

    /// Sends `request` and waits for its reply (other frames are dropped,
    /// text kept in [`console`](Self::console)); returns the reply and the
    /// round trip as the host saw it.
    pub fn call(&mut self, request: &Request) -> Result<(Message, Duration)> {
        let id = self.next_id;
        self.next_id += 1;
        let t0 = Instant::now();
        self.send(&request.encode(id))?;
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
    let us = |d: Duration| d.as_secs_f64() * 1e6;
    let mut samples: [(&str, Vec<f64>); 6] = [
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
    vm.shutdown();
    Ok(samples
        .into_iter()
        .map(|(name, mut values)| {
            let runs = values.len();
            let (median, p90) = summarize(&mut values);
            Metric {
                name: name.into(),
                guest: "rhizo".into(),
                vmm: Some(vmm.id().into()),
                unit: "us".into(),
                median,
                p90,
                runs,
            }
        })
        .collect())
}

mod expected {
    include!("../../cells/expected.rs");
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
        Ok((out.status.success(), stdout))
    };
    let fail = |what: &str, expected: &str| {
        let console = std::fs::read_to_string(state.join("console.log")).unwrap_or_default();
        Failure::new("control", what)
            .expected(expected)
            .actual(format!(
                "rhizo said:\n{}\nconsole:\n{console}",
                log.borrow()
            ))
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
    ])?;
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
    let _ = std::fs::remove_dir_all(&state);
    Ok(())
}
