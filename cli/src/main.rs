//! `rhizo`: boots a Rhizo VM with the control kernel and runs, lists,
//! suspends, resumes, forks, freezes and kills its cells, saves and loads
//! them as files, and talks to an agent in one (spec `control-plane`,
//! changes `add-control-plane`, `add-interactive-chat` and
//! `snapshots-on-disk`). Linux hosts only. Policy lives here: which
//! programs a template starts with which files, and when to suspend or
//! fork, are this tool's choices; the kernel only executes requests.

mod agent;
mod args;
mod audit;
mod chat;
mod client;
mod scope;
mod serve;
mod state;
mod tree;

use args::{Command, FileValue};
use client::{Client, Reply, duration};
use rhizo_control::{Body, Message, Placement, Request, Rule, Run, State};
use state::{Dir, Info};
use std::io::Write;
use std::process::{ExitCode, Stdio};
use std::time::{Duration, Instant};
use xtask::vmm::Vmm;

fn main() -> ExitCode {
    let words: Vec<String> = std::env::args().skip(1).collect();
    let result = args::parse(&words).and_then(|args| {
        let dir = Dir::resolve(args.state);
        match args.command {
            Command::Help => {
                print!("{}", args::USAGE);
                Ok(())
            }
            Command::Up(up) => cmd_up(&dir, &up),
            Command::Serve(up) => serve::serve(&dir, &up),
            Command::Down => cmd_down(&dir),
            Command::AuditTail { filter, follow } => audit_tail(&dir, &filter, follow),
            Command::AuditVerify => audit_verify(&dir),
            Command::SnapshotUpgrade { input, output } => snapshot_upgrade(&input, &output),
            Command::Run(run) if run.template == "python-agent" => {
                agent::run(&mut Client::connect(&dir)?, &run)
            }
            Command::Run(run) => cmd_run(&dir, &run),
            Command::Chat(chat) => chat::run(&mut Client::connect(&dir)?, &chat),
            Command::Ps => cmd_ps(&dir, false),
            Command::PsTree => cmd_ps(&dir, true),
            Command::Stats => cmd_stats(&dir),
            Command::Fork { id, n, place } => {
                let mut client = Client::connect(&dir)?;
                let request = match placement(&client, &place)? {
                    None => Request::fork(id, n),
                    Some(placement) => Request::Fork {
                        cell: id,
                        n,
                        placement,
                        watch: false,
                    },
                };
                let reply = client.call(&request)?;
                let Body::Cells(children) = &reply.body else {
                    return Err(format!("unexpected reply {:?}", reply.body));
                };
                println!("forked {id} into {children:?} in {}", reply.timing());
                Ok(())
            }
            Command::Suspend(id) => {
                let reply = Client::connect(&dir)?.call(&Request::Suspend(id))?;
                let bytes = match reply.body {
                    Body::Bytes(b) => b,
                    _ => 0,
                };
                println!(
                    "suspended {id} ({} KiB snapshot) in {}",
                    bytes / 1024,
                    reply.timing()
                );
                Ok(())
            }
            Command::Resume(id) => {
                let reply = Client::connect(&dir)?.call(&Request::Resume(id))?;
                println!("resumed {id} in {}", reply.timing());
                Ok(())
            }
            Command::SuspendTree(id) => subtree(&dir, &Request::SuspendTree(id), "suspended"),
            Command::ResumeTree(id) => subtree(&dir, &Request::ResumeTree(id), "resumed"),
            Command::KillTree(id) => {
                let reply = Client::connect(&dir)?.call(&Request::KillTree(id))?;
                println!("killed {id} and its subtree in {}", reply.timing());
                Ok(())
            }
            Command::Reparent { id, to } => {
                let reply = Client::connect(&dir)?.call(&Request::Reparent { cell: id, to })?;
                match to {
                    0 => println!("made {id} a root in {}", reply.timing()),
                    to => println!("moved {id} under {to} in {}", reply.timing()),
                }
                Ok(())
            }
            Command::Policy { id, place } => policy(&mut Client::connect(&dir)?, id, &place),
            Command::Scope { id, socket } => scope::serve(&dir, id, &socket),
            Command::Freeze(id) => {
                let reply = Client::connect(&dir)?.call(&Request::Freeze(id))?;
                println!("froze {id} into a template in {}", reply.timing());
                Ok(())
            }
            Command::Save { id, file } => save(&mut Client::connect(&dir)?, id, &file),
            Command::Load {
                file,
                template,
                place,
            } => load(&mut Client::connect(&dir)?, &file, template, &place),
            Command::Kill(id) => {
                let reply = Client::connect(&dir)?.call(&Request::Kill(id))?;
                println!("killed {id} in {}", reply.timing());
                Ok(())
            }
            Command::Attach(id) => attach(&mut Client::connect(&dir)?, id),
        }
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rhizo: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `rhizo audit tail`: the records of the log, then, with `follow`, new
/// ones as the background server appends them.
fn audit_tail(dir: &Dir, filter: &audit::Filter, follow: bool) -> Result<(), String> {
    let log = audit::dir(&dir.0);
    let mut from = 0;
    loop {
        let (shown, lines) = audit::tail(&log, filter, from)?;
        for line in shown {
            println!("{line}");
        }
        if !follow {
            return Ok(());
        }
        from = lines;
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// `rhizo audit verify`: fails if the chain is broken anywhere.
fn audit_verify(dir: &Dir) -> Result<(), String> {
    let log = audit::dir(&dir.0);
    let report = audit::verify(&log)?;
    for problem in &report.problems {
        println!("{problem}");
    }
    if report.dropped > 0 {
        println!(
            "the kernel dropped {} records, its ring being full (the host read too slowly)",
            report.dropped
        );
    }
    match report.problems.len() {
        0 => {
            println!(
                "{} records, chain intact ({})",
                report.records,
                log.display()
            );
            Ok(())
        }
        n => Err(format!("{n} problems in the audit log ({})", log.display())),
    }
}

/// `rhizo up`: builds the control kernel unless given, starts the
/// background server with it and waits until the kernel answers.
fn cmd_up(dir: &Dir, up: &args::Up) -> Result<(), String> {
    if let Some(info) = Info::read(dir) {
        return Err(format!(
            "a VM is already up ({} on {}, state {}): `rhizo down` first",
            info.vmm,
            info.arch,
            dir.0.display()
        ));
    }
    // What a server that did not end cleanly left.
    dir.clean();
    dir.create()?;
    xtask::arch::select(up.arch);
    let vmm = serve::vmm_of(up)?;
    if let Some(reason) = vmm.unavailable_reason(up.accel) {
        return Err(format!("{} is not available: {reason}", vmm.id()));
    }
    let kernel = match &up.kernel {
        Some(path) => path.clone(),
        None => xtask::build_kernel(xtask::Profile::Release, &["control"])?,
    };
    std::fs::copy(&kernel, dir.kernel())
        .map_err(|e| format!("cannot copy {}: {e}", kernel.display()))?;
    let log = std::fs::File::create(dir.log())
        .map_err(|e| format!("cannot create {}: {e}", dir.log().display()))?;
    let err = log.try_clone().map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--state")
        .arg(&dir.0)
        .arg("__serve")
        .args(["--vmm", vmm.id(), "--arch", up.arch.id()])
        .args(["--memory", &up.memory_mib.to_string()])
        .args(["--cpus", &up.cpus.to_string()])
        .args(["--accel", up.accel.id()])
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(err);
    // Its own process group: the terminal's Ctrl-C does not reach it.
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut server = cmd
        .spawn()
        .map_err(|e| format!("cannot start the server: {e}"))?;
    let deadline = Instant::now() + xtask::control::BOOT_TIMEOUT + Duration::from_secs(30);
    let info = loop {
        if let Some(info) = Info::read(dir) {
            break info;
        }
        if let Ok(Some(status)) = server.try_wait() {
            let log = std::fs::read_to_string(dir.log()).unwrap_or_default();
            return Err(format!("the VM did not start ({status}):\n{log}"));
        }
        if Instant::now() > deadline {
            let _ = server.kill();
            return Err("the VM did not answer in time".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let reply = Client::connect(dir)?.call(&Request::List)?;
    println!(
        "rhizo is up: {} ({}), {} MiB, {} CPU{}, kernel answering {} ms after launch; \
         control round trip {}; state in {}",
        info.vmm,
        info.arch,
        info.memory_mib,
        info.cpus,
        if info.cpus == 1 { "" } else { "s" },
        info.boot_ms,
        duration(reply.round_trip.as_nanos() as u64),
        dir.0.display()
    );
    Ok(())
}

/// `rhizo snapshot upgrade`: converts a file saved by the kernel of the
/// previous format version to the current one, checks it as a kernel
/// would, and writes it (design D20 of `add-cell-hierarchy`).
fn snapshot_upgrade(input: &std::path::Path, output: &std::path::Path) -> Result<(), String> {
    let bytes =
        std::fs::read(input).map_err(|e| format!("cannot read {}: {e}", input.display()))?;
    let version = |b: &[u8]| {
        b.get(8..12)
            .map(|v| u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
    };
    let from = version(&bytes);
    let upgraded = rhizo_snapshot::upgrade_file(bytes, rhizo_snapshot::crc32c).map_err(|e| {
        format!(
            "{}: cannot convert it ({e:?}); this rhizo converts format version {} to {}",
            input.display(),
            rhizo_snapshot::PREVIOUS,
            rhizo_snapshot::VERSION
        )
    })?;
    std::fs::write(output, &upgraded)
        .map_err(|e| format!("cannot write {}: {e}", output.display()))?;
    println!(
        "upgraded {} (format version {}) to {} (version {})",
        input.display(),
        from.unwrap_or(0),
        output.display(),
        rhizo_snapshot::VERSION
    );
    Ok(())
}

/// `rhizo down`: asks the kernel to power off and waits for the server,
/// which leaves only the console log in the state directory.
fn cmd_down(dir: &Dir) -> Result<(), String> {
    let mut client = Client::connect(dir)?;
    let t0 = Instant::now();
    let reply = client.call(&Request::Shutdown).map_err(|e| {
        format!(
            "the kernel did not answer the power off ({e}); its console log is in {}",
            dir.0.display()
        )
    })?;
    // The server removes the state, last, once the VMM has ended.
    let gone = |within: Duration| {
        let deadline = Instant::now() + within;
        while dir.state().exists() {
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        true
    };
    if !gone(Duration::from_secs(10)) {
        // The VMM did not end with the kernel's power off: end it.
        let _ = std::process::Command::new("kill")
            .arg(client.info.vmm_pid.to_string())
            .status();
        if !gone(Duration::from_secs(5)) {
            return Err("the VM did not stop".into());
        }
    }
    println!(
        "rhizo is down: power off requested in {}, VM gone {} after the request",
        reply.timing(),
        duration(t0.elapsed().as_nanos() as u64)
    );
    Ok(())
}

/// The request that starts `run`'s template.
fn template(run: &args::Run) -> Result<Run, String> {
    let (image, files_from, argv0) = match run.template.as_str() {
        "python" => ("cpython", "cpython-files", "/usr/bin/python3"),
        name => (name, "", name),
    };
    let mut files = Vec::new();
    for (path, value) in &run.files {
        let data = match value {
            FileValue::Text(text) => text.clone().into_bytes(),
            FileValue::Host(host) => std::fs::read(host)
                .map_err(|e| format!("--file {path}: cannot read {}: {e}", host.display()))?,
        };
        files.push((path.clone().into_bytes(), data));
    }
    Ok(Run {
        image: image.as_bytes().to_vec(),
        files_from: files_from.as_bytes().to_vec(),
        argv: std::iter::once(argv0)
            .chain(run.args.iter().map(String::as_str))
            .map(|a| a.as_bytes().to_vec())
            .collect(),
        envp: run.env.iter().map(|e| e.clone().into_bytes()).collect(),
        files,
        ..Run::default()
    })
}

/// The rule of the model endpoint `--llm` names, as the guest reaches it.
fn model_rule(client: &Client, llm: Option<&str>) -> Result<Option<Rule>, String> {
    llm.map(|url| {
        agent::guest_endpoint(url, Vmm::parse(&client.info.vmm)?)
            .and_then(|e| tree::endpoint_rule(&e))
    })
    .transpose()
}

/// The placement `place` gives, `None` for the defaults (which version 3
/// says too).
fn placement(client: &Client, place: &tree::Place) -> Result<Option<Placement>, String> {
    if place.is_default() {
        return Ok(None);
    }
    let model = model_rule(client, place.llm.as_deref())?;
    place.placement(model).map(Some)
}

fn cmd_run(dir: &Dir, run: &args::Run) -> Result<(), String> {
    let mut client = Client::connect(dir)?;
    let place = tree::Place {
        llm: run.llm.clone(),
        ..run.place.clone()
    };
    // Attached, its output comes right after the reply (protocol 4).
    let watch = !run.detach && client.current();
    let request = Run {
        placement: placement(&client, &place)?.unwrap_or_else(Placement::root),
        watch,
        ..template(run)?
    };
    let reply = client.call(&Request::Run(request))?;
    let &Body::Cell(id) = &reply.body else {
        return Err(format!("unexpected reply {:?}", reply.body));
    };
    println!("[rhizo] cell {id} started in {}", reply.timing());
    if run.detach {
        return Ok(());
    }
    attach_to(&mut client, id, watch.then_some(reply.id))
}

/// Shows cell `id`'s output until it ends.
fn attach(client: &mut Client, id: u32) -> Result<(), String> {
    attach_to(client, id, None)
}

/// Shows cell `id`'s output until it ends, as request `watch` streams it,
/// or an `output` request when `None`.
fn attach_to(client: &mut Client, id: u32, watch: Option<u32>) -> Result<(), String> {
    let watch = match watch {
        Some(w) => w,
        None => client.send(&Request::Output(id))?,
    };
    let mut out = std::io::stdout();
    loop {
        match client.next()? {
            (w, Message::Output { data, .. }) if w == watch => {
                let _ = out.write_all(&data);
                let _ = out.flush();
            }
            (w, Message::Ended { state, .. }) if w == watch => {
                println!("[rhizo] cell {id} {}", describe(state));
                return Ok(());
            }
            (w, Message::Failed { error, .. }) if w == watch => {
                return Err(format!("cell {id}: {}", error.as_str()));
            }
            _ => {}
        }
    }
}

/// `rhizo save`: the snapshot comes over TCP through the VM's network card
/// (design D2 of `snapshots-on-disk`), and is written once all came.
fn save(client: &mut Client, id: u32, file: &std::path::Path) -> Result<(), String> {
    let transfer = xtask::control::Transfer::new(Vmm::parse(&client.info.vmm)?)?;
    let host = transfer.host;
    let received = transfer.receive();
    let reply = client.call(&Request::Save { cell: id, host })?;
    let bytes = received.join().map_err(|_| "the transfer panicked")??;
    if reply.body != Body::Bytes(bytes.len() as u64) {
        return Err(format!(
            "received {} bytes, the kernel sent {:?}",
            bytes.len(),
            reply.body
        ));
    }
    let tmp = file.with_extension("partial");
    std::fs::write(&tmp, &bytes)
        .and_then(|()| std::fs::rename(&tmp, file))
        .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    println!(
        "saved {id} to {} ({}) in {}, {}",
        file.display(),
        size(bytes.len()),
        reply.timing(),
        rate(bytes.len(), reply.round_trip)
    );
    Ok(())
}

/// `rhizo load`: the file goes over TCP through the VM's network card.
fn load(
    client: &mut Client,
    file: &std::path::Path,
    template: bool,
    place: &tree::Place,
) -> Result<(), String> {
    let placement = placement(client, place)?.unwrap_or_else(Placement::root);
    let bytes = std::fs::read(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let n = bytes.len();
    let transfer = xtask::control::Transfer::new(Vmm::parse(&client.info.vmm)?)?;
    let host = transfer.host;
    let sent = transfer.send(bytes);
    let reply = client
        .call(&Request::Load {
            size: n as u64,
            template,
            host,
            placement,
        })
        .map_err(|e| format!("{}: {e}", file.display()))?;
    sent.join().map_err(|_| "the transfer panicked")??;
    let &Body::Cell(cell) = &reply.body else {
        return Err(format!("unexpected reply {:?}", reply.body));
    };
    println!(
        "loaded {} as {} {cell} ({}) in {}, {}",
        file.display(),
        if template {
            "template"
        } else {
            "suspended cell"
        },
        size(n),
        reply.timing(),
        rate(n, reply.round_trip)
    );
    Ok(())
}

fn refusal(error: rhizo_control::Error, message: &[u8]) -> String {
    if message.is_empty() {
        error.as_str().to_string()
    } else {
        format!("{}: {}", error.as_str(), String::from_utf8_lossy(message))
    }
}

fn size(bytes: usize) -> String {
    format!("{:.1} MiB", bytes as f64 / (1 << 20) as f64)
}

fn rate(bytes: usize, took: Duration) -> String {
    format!(
        "{:.2} MiB/s",
        bytes as f64 / (1 << 20) as f64 / took.as_secs_f64().max(1e-9)
    )
}

fn describe(state: State) -> String {
    match state {
        State::Running => "running".into(),
        State::Suspended { bytes } => format!("suspended ({} KiB)", bytes / 1024),
        State::Exited(code) => format!("exited {code}"),
        State::Fault => "faulted".into(),
        State::Killed => "killed".into(),
        State::Template => "template".into(),
    }
}

fn cmd_ps(dir: &Dir, as_tree: bool) -> Result<(), String> {
    let mut client = Client::connect(dir)?;
    if as_tree && !client.current() {
        return Err(format!(
            "the VM's kernel speaks control protocol {}, which has no cell tree; `rhizo ps` \
             lists its cells",
            client.protocol
        ));
    }
    let reply: Reply = client.call(&Request::List)?;
    let Body::List(cells) = &reply.body else {
        return Err(format!("unexpected reply {:?}", reply.body));
    };
    if as_tree {
        for row in tree::tree_rows(cells, |c| describe(c.state)) {
            println!("{row}");
        }
        println!("({} cells, listed in {})", cells.len(), reply.timing());
        return Ok(());
    }
    println!("{:<6} {:<22} IMAGE", "ID", "STATE");
    for c in cells {
        println!(
            "{:<6} {:<22} {}",
            c.id,
            describe(c.state),
            String::from_utf8_lossy(&c.image)
        );
    }
    println!("({} cells, listed in {})", cells.len(), reply.timing());
    Ok(())
}

/// `suspend --tree` or `resume --tree`: the cells it reached.
fn subtree(dir: &Dir, request: &Request, done: &str) -> Result<(), String> {
    let reply = Client::connect(dir)?.call(request)?;
    let Body::Cells(cells) = &reply.body else {
        return Err(format!("unexpected reply {:?}", reply.body));
    };
    println!("{done} {cells:?} in {}", reply.timing());
    Ok(())
}

/// `rhizo policy`: the egress and ceiling given, then the limits given
/// (those not given stay as they are).
fn policy(client: &mut Client, id: u32, place: &tree::Place) -> Result<(), String> {
    let model = model_rule(client, place.llm.as_deref())?;
    if let Some(egress) = &place.egress {
        let ceiling = place
            .ceiling
            .as_ref()
            .map(|c| c.resolve(model))
            .transpose()?;
        let reply = client.call(&Request::Policy {
            cell: id,
            egress: egress.resolve(model)?,
            ceiling,
        })?;
        println!("set {id}'s egress in {}", reply.timing());
    }
    if place.has_limits() {
        let reply = client.call(&Request::List)?;
        let Body::List(cells) = reply.body else {
            return Err(format!("unexpected reply {:?}", reply.body));
        };
        let now = cells
            .iter()
            .find(|c| c.id == id)
            .and_then(|c| c.tree.as_ref())
            .ok_or_else(|| format!("cell {id} is not in the tree"))?
            .limits;
        let limits = rhizo_control::Limits {
            memory: place.memory.unwrap_or(now.memory),
            cpu_ns: place.cpu_ns.unwrap_or(now.cpu_ns),
            sockets: place.sockets.unwrap_or(now.sockets),
            cells: place.cells.unwrap_or(now.cells),
        };
        let reply = client.call(&Request::Limits { cell: id, limits })?;
        println!(
            "set {id}'s limits to {} in {}",
            tree::show_limits(&limits),
            reply.timing()
        );
    }
    Ok(())
}

fn cmd_stats(dir: &Dir) -> Result<(), String> {
    let reply = Client::connect(dir)?.call(&Request::Stats)?;
    let Body::Stats(s) = &reply.body else {
        return Err(format!("unexpected reply {:?}", reply.body));
    };
    let mib = |b: u64| b as f64 / (1 << 20) as f64;
    println!(
        "cells: {} running, {} suspended ({:.1} MiB of snapshots), {} templates, {} ended\n\
         memory: {:.1} MiB free of {:.1} MiB\n(in {})",
        s.running,
        s.suspended,
        mib(s.snapshot_bytes),
        s.templates,
        s.ended,
        mib(s.memory_free),
        mib(s.memory_total),
        reply.timing()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    /// `rhizo snapshot upgrade` converts a file a version-10 kernel saved
    /// (recorded in `rhizo-snapshot`'s test data) into one the current
    /// kernel loads, leaves a current file as it is, and refuses a damaged
    /// one (design D20 of `add-cell-hierarchy`).
    #[test]
    fn snapshot_upgrade_converts_a_version_10_file() {
        let dir = std::env::temp_dir().join(format!("rhizo-upgrade-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (old, new) = (dir.join("old.rhz"), dir.join("new.rhz"));
        let v10 = include_bytes!("../../crates/rhizo-snapshot/tests/data/version-10.rhz");
        std::fs::write(&old, v10).unwrap();
        super::snapshot_upgrade(&old, &new).unwrap();
        let bytes = std::fs::read(&new).unwrap();
        let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        assert_eq!(version, rhizo_snapshot::VERSION);
        let again = dir.join("again.rhz");
        super::snapshot_upgrade(&new, &again).unwrap();
        assert_eq!(
            std::fs::read(&again).unwrap(),
            bytes,
            "a current file stays"
        );
        let mut bad = v10.to_vec();
        bad[40] ^= 1;
        std::fs::write(&old, bad).unwrap();
        let e = super::snapshot_upgrade(&old, &new).unwrap_err();
        assert!(e.contains("Checksum"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
