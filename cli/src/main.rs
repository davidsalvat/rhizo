//! `rhizo`: boots a Rhizo VM with the control kernel and runs, lists,
//! suspends, resumes, forks and kills its cells, and talks to an agent in
//! one (spec `control-plane`, changes `add-control-plane` and
//! `add-interactive-chat`). Linux hosts only. Policy lives here: which
//! programs a template starts with which files, and when to suspend or
//! fork, are this tool's choices; the kernel only executes requests.

mod agent;
mod args;
mod chat;
mod client;
mod serve;
mod state;

use args::{Command, FileValue};
use client::{Client, Reply, duration};
use rhizo_control::{Body, Message, Request, Run, State};
use state::{Dir, Info};
use std::io::Write;
use std::process::{ExitCode, Stdio};
use std::time::{Duration, Instant};

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
            Command::Run(run) if run.template == "python-agent" => {
                agent::run(&mut Client::connect(&dir)?, &run)
            }
            Command::Run(run) => cmd_run(&dir, &run),
            Command::Chat(chat) => chat::run(&mut Client::connect(&dir)?, &chat),
            Command::Ps => cmd_ps(&dir),
            Command::Stats => cmd_stats(&dir),
            Command::Fork { id, n } => {
                let reply = Client::connect(&dir)?.call(&Request::Fork { cell: id, n })?;
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
    std::fs::create_dir_all(&dir.0)
        .map_err(|e| format!("cannot create {}: {e}", dir.0.display()))?;
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

/// `rhizo down`: asks the kernel to power off and waits for the server.
fn cmd_down(dir: &Dir) -> Result<(), String> {
    let mut client = Client::connect(dir)?;
    let t0 = Instant::now();
    let reply = client.call(&Request::Shutdown)?;
    // The server removes the state once the VMM has ended.
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
        input: false,
    })
}

fn cmd_run(dir: &Dir, run: &args::Run) -> Result<(), String> {
    let mut client = Client::connect(dir)?;
    let reply = client.call(&Request::Run(template(run)?))?;
    let &Body::Cell(id) = &reply.body else {
        return Err(format!("unexpected reply {:?}", reply.body));
    };
    println!("[rhizo] cell {id} started in {}", reply.timing());
    if run.detach {
        return Ok(());
    }
    attach(&mut client, id)
}

/// Shows cell `id`'s output until it ends.
fn attach(client: &mut Client, id: u32) -> Result<(), String> {
    let watch = client.send(&Request::Output(id))?;
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

fn describe(state: State) -> String {
    match state {
        State::Running => "running".into(),
        State::Suspended { bytes } => format!("suspended ({} KiB)", bytes / 1024),
        State::Exited(code) => format!("exited {code}"),
        State::Fault => "faulted".into(),
        State::Killed => "killed".into(),
    }
}

fn cmd_ps(dir: &Dir) -> Result<(), String> {
    let reply: Reply = Client::connect(dir)?.call(&Request::List)?;
    let Body::List(cells) = &reply.body else {
        return Err(format!("unexpected reply {:?}", reply.body));
    };
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

fn cmd_stats(dir: &Dir) -> Result<(), String> {
    let reply = Client::connect(dir)?.call(&Request::Stats)?;
    let Body::Stats(s) = &reply.body else {
        return Err(format!("unexpected reply {:?}", reply.body));
    };
    let mib = |b: u64| b as f64 / (1 << 20) as f64;
    println!(
        "cells: {} running, {} suspended ({:.1} MiB of snapshots), {} ended\n\
         memory: {:.1} MiB free of {:.1} MiB\n(in {})",
        s.running,
        s.suspended,
        mib(s.snapshot_bytes),
        s.ended,
        mib(s.memory_free),
        mib(s.memory_total),
        reply.timing()
    );
    Ok(())
}
