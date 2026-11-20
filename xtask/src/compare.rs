//! Comparisons with existing isolation mechanisms, measured on the same
//! machine as the cells (spec `comparisons`):
//!
//! - Linux processes: `fork` of a process with 50 MiB written, and spawning
//!   the same static `hello` program the cell benchmark runs, on the host
//!   and inside a Firecracker guest (the same VM the cell figures come from).
//! - Firecracker snapshots: a Linux guest is snapshotted through
//!   Firecracker's API, then restored from scratch (new VMM process) until
//!   the VM runs again: the state-of-the-art suspend/resume of a sandbox.

use crate::bench::Metric;
use crate::vmm::{Accel, Vmm};
use crate::{Result, arch, refguest, report, tools, workspace_root};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

fn metric(name: &str, guest: &str, vmm: &str, unit: &str, values: &mut [f64]) -> Metric {
    let (median, p90) = crate::bench::summarize(values);
    Metric {
        name: name.into(),
        guest: guest.into(),
        vmm: Some(vmm.into()),
        unit: unit.into(),
        median,
        p90,
        runs: values.len(),
    }
}

/// The fields `bench/host/linux_process.c` prints, in this order.
const PROCESS_FIELDS: [&str; 4] = ["fork_us", "fork_call_us", "fork_touch_us", "spawn_us"];

/// Parses a `linux-process key=value...` line into [`PROCESS_FIELDS`].
pub fn parse_process_line(text: &str) -> Option<[f64; 4]> {
    let line = text.lines().find_map(|l| {
        let at = l.find("linux-process ")?;
        Some(&l[at..])
    })?;
    let field = |key: &str| -> Option<f64> {
        line.split_whitespace()
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?
            .parse()
            .ok()
    };
    let mut out = [0.0; 4];
    for (slot, key) in out.iter_mut().zip(PROCESS_FIELDS) {
        *slot = field(key)?;
    }
    Some(out)
}

/// One metric per [`PROCESS_FIELDS`] entry, from several result lines.
fn process_metrics(vmm: &str, samples: &[[f64; 4]]) -> Vec<Metric> {
    PROCESS_FIELDS
        .iter()
        .enumerate()
        .map(|(i, key)| {
            let mut values: Vec<f64> = samples.iter().map(|s| s[i]).collect();
            metric(
                &format!("process.{key}"),
                "linux-process",
                vmm,
                "us",
                &mut values,
            )
        })
        .collect()
}

/// Compiles `bench/host/linux_process.c` into `binary` with `cc` and
/// extra flags.
fn build_linux_process(cc: &str, flags: &[&str], binary: &Path) -> Result {
    let source = workspace_root().join("bench/host/linux_process.c");
    let status = Command::new(cc)
        .args(flags)
        .args(["-O2", "-o"])
        .arg(binary)
        .arg(&source)
        .status()
        .map_err(|e| format!("cannot run {cc}: {e}"))?;
    if !status.success() {
        return Err(format!("cannot compile {}", source.display()));
    }
    Ok(())
}

/// Linux process baselines on this host, running the cell `hello` (from
/// `cells`) natively. Needs a Linux host of the target architecture.
pub fn linux_process(cells: &Path, scratch: &Path) -> Result<Vec<Metric>> {
    if crate::differential::unavailable_reason().is_some() {
        return Err("the host cannot run the cells natively".into());
    }
    let binary = scratch.join("linux_process");
    build_linux_process("cc", &[], &binary)?;
    let mut samples = Vec::new();
    for _ in 0..3 {
        let out = Command::new(&binary)
            .arg(cells.join("hello"))
            .stderr(Stdio::inherit())
            .output()
            .map_err(|e| format!("cannot run {}: {e}", binary.display()))?;
        let text = String::from_utf8_lossy(&out.stdout);
        samples.push(parse_process_line(&text).ok_or("linux_process printed no result line")?);
    }
    Ok(process_metrics("host", &samples))
}

/// The same Linux process baselines inside a Firecracker guest: the VM
/// Rhizo's own cell figures come from (design D1 of `add-fair-comparisons`).
/// The static program runs as the guest's `/init`, next to `/hello`.
pub fn linux_process_in_vm(cells: &Path, accel: Accel, scratch: &Path) -> Result<Vec<Metric>> {
    if let Some(reason) = Vmm::Firecracker.unavailable_reason(accel) {
        return Err(reason);
    }
    let program = scratch.join("linux_process_static");
    build_linux_process(crate::c_compiler(), &["-static"], &program)?;
    let initramfs =
        refguest::initramfs_with(&read(&program)?, &[("hello", &read(&cells.join("hello"))?)]);
    let samples = boot_guest(&initramfs, scratch, "linux-process", parse_process_line)?;
    report::text(&format!(
        "linux processes inside firecracker on {}: {} boots",
        arch::current().id(),
        samples.len()
    ));
    Ok(process_metrics("firecracker", &samples))
}

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// Boots a Linux guest in Firecracker with `initramfs` three times and
/// returns what `parse` finds in the first console line it accepts, per
/// boot (`what` names the line in errors).
fn boot_guest<T: Send + 'static>(
    initramfs: &[u8],
    scratch: &Path,
    what: &str,
    parse: fn(&str) -> Option<T>,
) -> Result<Vec<T>> {
    let binary = tools::firecracker()?;
    let kernel = tools::reference_kernel()?;
    let image = scratch.join(format!("{what}-{}.cpio", std::process::id()));
    std::fs::write(&image, initramfs).map_err(|e| e.to_string())?;
    let config = scratch.join(format!("{what}-{}.json", std::process::id()));
    std::fs::write(
        &config,
        format!(
            "{{\"boot-source\": {{\"kernel_image_path\": {:?}, \"boot_args\": {:?}, \
             \"initrd_path\": {:?}}}, \"drives\": [], \
             \"machine-config\": {{\"vcpu_count\": 1, \"mem_size_mib\": 256}}}}",
            kernel.display().to_string(),
            crate::vmm::linux_cmdline(Vmm::Firecracker),
            image.display().to_string()
        ),
    )
    .map_err(|e| e.to_string())?;
    let mut samples = Vec::new();
    for boot in 0..3 {
        let mut child = Command::new(&binary)
            .arg("--no-api")
            .arg("--config-file")
            .arg(&config)
            .args(["--level", "Error"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start Firecracker: {e}"))?;
        let stdout = child.stdout.take().ok_or("no Firecracker output")?;
        let (tx, rx) = std::sync::mpsc::channel();
        let console = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = console.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
                if let Some(sample) = parse(&line) {
                    let _ = tx.send(sample);
                }
                if let Ok(mut lines) = seen.lock() {
                    lines.push(line);
                }
            }
        });
        let sample = rx.recv_timeout(Duration::from_secs(120));
        stop(child);
        samples.push(sample.map_err(|_| {
            let lines = console.lock().map(|l| l.clone()).unwrap_or_default();
            let tail = &lines[lines.len().saturating_sub(15)..];
            format!(
                "boot {boot}: the guest printed no {what} line within 120 s; its console ended \
                 with:\n{}",
                tail.join("\n")
            )
        })?);
    }
    let _ = std::fs::remove_file(&image);
    let _ = std::fs::remove_file(&config);
    Ok(samples)
}

/// Parses a `linux-cpython cold_us=<x> warm_us=<y>` line.
pub fn parse_cpython_line(text: &str) -> Option<[f64; 2]> {
    let line = &text[text.find("linux-cpython ")?..];
    let field = |key: &str| -> Option<f64> {
        line.split_whitespace()
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?
            .parse()
            .ok()
    };
    Some([field("cold_us")?, field("warm_us")?])
}

/// CPython on Linux in the same Firecracker VM as the cells (design D4 of
/// `add-cpython`): the static CPython, its standard library and the script
/// of the CPython cell in the guest's initramfs, `bench/host/linux_cpython.c`
/// as `/init`. Cold: fork and exec of the script; warm: a fork of a CPython
/// process that imported and warmed it up (`cpython_forkserver.py`).
pub fn cpython_in_vm(accel: Accel, scratch: &Path) -> Result<Vec<Metric>> {
    let root = crate::cpython::root();
    if !root.join("usr/bin/python3").exists() {
        return Err("CPython not built (RHIZO_CPYTHON_SRC unset)".into());
    }
    if let Some(reason) = Vmm::Firecracker.unavailable_reason(accel) {
        return Err(reason);
    }
    let program = scratch.join("linux_cpython_static");
    let source = workspace_root().join("bench/host/linux_cpython.c");
    let status = Command::new(crate::c_compiler())
        .args(["-static", "-O2", "-o"])
        .arg(&program)
        .arg(&source)
        .status()
        .map_err(|e| format!("cannot run the C compiler: {e}"))?;
    if !status.success() {
        return Err(format!("cannot compile {}", source.display()));
    }
    let mut files = Vec::new();
    collect(&root, "", &mut files)?;
    let server = workspace_root().join("bench/host/cpython_forkserver.py");
    files.push(("app/cpython_forkserver.py".into(), read(&server)?));
    let entries: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    let initramfs = refguest::initramfs_with(&read(&program)?, &entries);
    let samples = boot_guest(&initramfs, scratch, "linux-cpython", parse_cpython_line)?;
    report::text(&format!(
        "cpython inside firecracker on {}: {} boots",
        arch::current().id(),
        samples.len()
    ));
    let mut out = Vec::new();
    for (i, name) in ["workload.cpython_cold_us", "workload.cpython_warm_us"]
        .into_iter()
        .enumerate()
    {
        let mut values: Vec<f64> = samples.iter().map(|s| s[i]).collect();
        out.push(metric(
            name,
            "linux-process",
            "firecracker",
            "us",
            &mut values,
        ));
    }
    Ok(out)
}

/// Every file and directory under `dir` as initramfs entries named from
/// `at` (directories end in `/`), in name order.
fn collect(dir: &Path, at: &str, out: &mut Vec<(String, Vec<u8>)>) -> Result {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = format!("{at}{}", entry.file_name().to_string_lossy());
        if entry.path().is_dir() {
            out.push((format!("{name}/"), Vec::new()));
            collect(&entry.path(), &format!("{name}/"), out)?;
        } else {
            out.push((name, read(&entry.path())?));
        }
    }
    Ok(())
}

/// One HTTP/1.1 request to Firecracker's API socket; returns the status.
fn api(socket: &Path, method: &str, path: &str, body: &str) -> Result<u16> {
    let mut stream =
        UnixStream::connect(socket).map_err(|e| format!("cannot connect to the API: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nAccept: application/json\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .map_err(|e| format!("cannot send to the API: {e}"))?;
    let mut buf = [0u8; 1024];
    let n = stream
        .read(&mut buf)
        .map_err(|e| format!("no API answer: {e}"))?;
    let head = String::from_utf8_lossy(&buf[..n]);
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad API answer: {head}"))?;
    if !(200..300).contains(&status) {
        return Err(format!("{method} {path} -> {status}: {head}"));
    }
    Ok(status)
}

/// Waits until the API socket accepts connections.
fn wait_for_socket(socket: &Path, deadline: Instant) -> Result {
    while Instant::now() < deadline {
        if UnixStream::connect(socket).is_ok() {
            return Ok(());
        }
        std::hint::spin_loop();
    }
    Err("Firecracker's API socket never came up".into())
}

/// Kills and reaps a VMM process.
fn stop(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Restores a snapshot of the reference Linux guest `runs` times; returns
/// `snapshot.restore_ms` (from launching Firecracker to the VM running
/// again) and `snapshot.restore_api_ms` (the load request alone).
pub fn firecracker_restore(runs: usize, accel: Accel, scratch: &Path) -> Result<Vec<Metric>> {
    if let Some(reason) = Vmm::Firecracker.unavailable_reason(accel) {
        return Err(reason);
    }
    let binary = tools::firecracker()?;
    let kernel = tools::reference_kernel()?;
    let initramfs = refguest::write_initramfs()?;
    let dir = scratch.join(format!("fc-snapshot-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (state, memory) = (dir.join("vm.state"), dir.join("vm.mem"));
    let socket = dir.join("api.sock");
    let config = dir.join("config.json");
    std::fs::write(
        &config,
        format!(
            "{{\"boot-source\": {{\"kernel_image_path\": {:?}, \"boot_args\": {:?}, \
             \"initrd_path\": {:?}}}, \"drives\": [], \
             \"machine-config\": {{\"vcpu_count\": 1, \"mem_size_mib\": 128}}}}",
            kernel.display().to_string(),
            crate::vmm::linux_cmdline(Vmm::Firecracker),
            initramfs.display().to_string()
        ),
    )
    .map_err(|e| e.to_string())?;

    // Boot the guest to its ready line, pause it, snapshot it.
    let _ = std::fs::remove_file(&socket);
    let mut boot = Command::new(&binary)
        .arg("--api-sock")
        .arg(&socket)
        .arg("--config-file")
        .arg(&config)
        .args(["--level", "Error"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start Firecracker: {e}"))?;
    let stdout = boot.stdout.take().ok_or("no Firecracker output")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
            if line.contains("rhizo-ready linux") {
                let _ = tx.send(());
            }
        }
    });
    if rx.recv_timeout(Duration::from_secs(30)).is_err() {
        stop(boot);
        return Err("the reference guest never became ready under Firecracker".into());
    }
    let snapshot = (|| {
        api(&socket, "PATCH", "/vm", "{\"state\": \"Paused\"}")?;
        api(
            &socket,
            "PUT",
            "/snapshot/create",
            &format!(
                "{{\"snapshot_type\": \"Full\", \"snapshot_path\": {:?}, \"mem_file_path\": {:?}}}",
                state.display().to_string(),
                memory.display().to_string()
            ),
        )
        .map(|_| ())
    })();
    stop(boot);
    snapshot?;

    let load = format!(
        "{{\"snapshot_path\": {:?}, \"mem_backend\": {{\"backend_type\": \"File\", \
         \"backend_path\": {:?}}}, \"resume_vm\": true}}",
        state.display().to_string(),
        memory.display().to_string()
    );
    let (mut total, mut request) = (Vec::new(), Vec::new());
    for _ in 0..=runs {
        let _ = std::fs::remove_file(&socket);
        let t0 = Instant::now();
        let child = Command::new(&binary)
            .arg("--api-sock")
            .arg(&socket)
            .args(["--level", "Error"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start Firecracker: {e}"))?;
        let restored = wait_for_socket(&socket, t0 + Duration::from_secs(10)).and_then(|()| {
            let t1 = Instant::now();
            api(&socket, "PUT", "/snapshot/load", &load)?;
            Ok((t0.elapsed(), t1.elapsed()))
        });
        stop(child);
        let (all, req) = restored?;
        total.push(all.as_secs_f64() * 1000.0);
        request.push(req.as_secs_f64() * 1000.0);
    }
    // The first restore warms the page cache; drop it like the boot runs do.
    total.remove(0);
    request.remove(0);
    let _ = std::fs::remove_dir_all(&dir);
    report::text(&format!(
        "firecracker snapshot restore on {}: {} runs",
        arch::current().id(),
        total.len()
    ));
    Ok(vec![
        metric(
            "snapshot.restore_ms",
            "linux",
            "firecracker",
            "ms",
            &mut total,
        ),
        metric(
            "snapshot.restore_api_ms",
            "linux",
            "firecracker",
            "ms",
            &mut request,
        ),
    ])
}

/// Where comparison builds and snapshots go.
pub fn scratch_dir() -> PathBuf {
    crate::target_dir().join("compare")
}

/// `program` run as root (`sudo -n` unless already root), stopped after
/// 60 s, with no standard input.
fn as_root(program: &Path) -> Command {
    let root = Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0");
    let mut cmd = if root {
        Command::new("timeout")
    } else {
        let mut c = Command::new("sudo");
        c.arg("-n").arg("timeout");
        c
    };
    cmd.args(["--kill-after=5", "60"]).arg(program);
    cmd.stdin(Stdio::null());
    cmd
}

/// A program on `PATH`, or the path in the environment variable `var`.
fn find_tool(var: &str, name: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(var) {
        return Some(PathBuf::from(path));
    }
    let out = Command::new("sh")
        .args(["-c", &format!("command -v {name}")])
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// The seconds of the last `(SS.ffffff)` timestamp in a CRIU log: the time
/// from CRIU's start to its last message.
pub fn criu_log_seconds(log: &str) -> Option<f64> {
    log.lines().rev().find_map(|l| {
        let rest = l.trim_start().strip_prefix('(')?;
        rest[..rest.find(')')?].trim().parse().ok()
    })
}

/// CRIU restore of a process holding as much written memory as the cell
/// snapshot (design D2 of `add-fair-comparisons`): dumped once, restored
/// `runs` times until it runs again. Each restore is the time CRIU reports
/// in its log from its start to "Restore finished" (the `sudo` and exec in
/// front of it are not counted).
pub fn criu(runs: usize, scratch: &Path) -> Result<Vec<Metric>> {
    let criu = find_tool("RHIZO_CRIU", "criu")
        .ok_or("CRIU is not installed (CI installs it; see bench/README.md)")?;
    let dir = scratch.join(format!("criu-{}", std::process::id()));
    let images = dir.join("images");
    std::fs::create_dir_all(&images).map_err(|e| e.to_string())?;
    let target = dir.join("criu_target");
    let source = workspace_root().join("bench/host/criu_target.c");
    let built = Command::new("cc")
        .args(["-O2", "-o"])
        .arg(&target)
        .arg(&source)
        .status()
        .map_err(|e| format!("cannot run cc: {e}"))?;
    if !built.success() {
        return Err(format!("cannot compile {}", source.display()));
    }
    let run = |args: &[&str], what: &str| -> Result<()> {
        let status = as_root(&criu)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| format!("cannot run criu: {e}"))?;
        if status.success() {
            return Ok(());
        }
        let log = images.join(format!("{what}.log"));
        let text = Command::new("sudo")
            .args(["-n", "cat"])
            .arg(&log)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let why: Vec<&str> = text
            .lines()
            .filter(|l| l.contains("Error"))
            .take(8)
            .collect();
        Err(format!("criu {what} failed ({status}): {}", why.join("\n")))
    };
    // The target: its own session (no terminal to carry), output to a file.
    let out = dir.join("target.out");
    let file = std::fs::File::create(&out).map_err(|e| e.to_string())?;
    let mut child = Command::new("setsid")
        .arg(&target)
        .arg("8")
        .stdin(Stdio::null())
        .stdout(file)
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start the CRIU target: {e}"))?;
    let pid = child.id();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !std::fs::read_to_string(&out)
        .unwrap_or_default()
        .contains("ready")
    {
        if Instant::now() > deadline {
            let _ = child.kill();
            return Err("the CRIU target never became ready".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let images_arg = images.display().to_string();
    let dumped = run(
        &[
            "dump",
            "-t",
            &pid.to_string(),
            "-D",
            &images_arg,
            "-o",
            "dump.log",
            "-v4",
        ],
        "dump",
    );
    let _ = child.kill();
    let _ = child.wait();
    dumped?;
    let mut restore = Vec::new();
    for i in 0..=runs {
        // CRIU refuses to overwrite a pidfile: a new one for every restore.
        let pidfile = dir.join(format!("restored-{i}.pid")).display().to_string();
        run(
            &[
                "restore",
                "-d",
                "-D",
                &images_arg,
                "-o",
                "restore.log",
                "-v4",
                "--pidfile",
                &pidfile,
            ],
            "restore",
        )?;
        let log = Command::new("sudo")
            .args(["-n", "cat"])
            .arg(images.join("restore.log"))
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        let seconds = criu_log_seconds(&log).ok_or("the CRIU restore log has no timestamp")?;
        // The restored process has the dumped pid: kill it, and wait until
        // the pid is free for the next restore.
        let _ = Command::new("sudo")
            .args(["-n", "kill", "-9", &pid.to_string()])
            .status();
        let gone = Instant::now() + Duration::from_secs(10);
        while Path::new(&format!("/proc/{pid}")).exists() && Instant::now() < gone {
            std::thread::sleep(Duration::from_millis(5));
        }
        if i > 0 {
            restore.push(seconds * 1000.0);
        }
    }
    let _ = Command::new("sudo")
        .args(["-n", "rm", "-rf"])
        .arg(&dir)
        .status();
    report::text(&format!("criu restore: {} runs", restore.len()));
    Ok(vec![metric(
        "snapshot.restore_ms",
        "criu",
        "host",
        "ms",
        &mut restore,
    )])
}

/// The `runsc` binary: `RHIZO_RUNSC`, or `runsc` on `PATH`.
fn runsc() -> Result<PathBuf> {
    find_tool("RHIZO_RUNSC", "runsc").ok_or_else(|| {
        "gVisor's runsc is not installed (CI installs it; see bench/README.md)".into()
    })
}

/// A `runsc` command run as root (`sudo -n` unless already root), stopped
/// after 60 s: gVisor needs root for `do`, `run`, `checkpoint` and `restore`.
fn runsc_cmd(runsc: &Path, root: &Path) -> Command {
    let as_root = Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0");
    // Every command is bounded: a gVisor that hangs on this host becomes a
    // skipped comparison with the reason, not a stuck benchmark.
    let mut cmd = if as_root {
        Command::new("timeout")
    } else {
        let mut c = Command::new("sudo");
        c.arg("-n").arg("timeout");
        c
    };
    cmd.args(["--kill-after=5", "60"]).arg(runsc);
    // gVisor's default platform (systrap) unless `RHIZO_GVISOR_PLATFORM` says
    // otherwise (`kvm`); cgroups are left alone (CI runners differ); every
    // command logs to `<root>/../logs/`, quoted when a command fails.
    let mut logs = root
        .parent()
        .unwrap_or(root)
        .join("logs")
        .display()
        .to_string();
    logs.push('/');
    cmd.arg("--root")
        .arg(root)
        .arg(format!(
            "--platform={}",
            std::env::var("RHIZO_GVISOR_PLATFORM").unwrap_or_else(|_| "systrap".into())
        ))
        .args(["--network=none", "--ignore-cgroups", "--debug"])
        .arg(format!("--debug-log={logs}"));
    cmd.stdin(Stdio::null());
    cmd
}

/// Runs `cmd`; on failure, its output and the end of gVisor's newest debug
/// logs (in `logs`) as the error.
fn checked(cmd: &mut Command, what: &str, logs: &Path) -> Result<String> {
    // Output goes to files, not pipes: the sandbox and gofer `runsc` starts
    // inherit them, and a pipe would stay open (and `output()` wait) for as
    // long as they live, even after `timeout` killed `runsc` itself.
    // New files for every command: runsc hands its standard output and error
    // to the container's user (root), so a file it was given cannot be
    // reopened by this process afterwards. Read back with `sudo cat` if needed.
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let base = std::env::temp_dir().join(format!("rhizo-runsc-{}-{n}", std::process::id()));
    let (out_path, err_path) = (base.with_extension("out"), base.with_extension("err"));
    let file = |p: &Path| std::fs::File::create(p).map_err(|e| format!("{}: {e}", p.display()));
    let status = cmd
        .stdout(file(&out_path)?)
        .stderr(file(&err_path)?)
        .status()
        .map_err(|e| format!("cannot run runsc ({what}): {e}"))?;
    let read = |p: &Path| {
        std::fs::read(p).unwrap_or_else(|_| {
            Command::new("sudo")
                .args(["-n", "cat"])
                .arg(p)
                .output()
                .map(|o| o.stdout)
                .unwrap_or_default()
        })
    };
    let out = Output {
        status,
        stdout: read(&out_path),
        stderr: read(&err_path),
    };
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let mut files: Vec<_> = std::fs::read_dir(logs)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort();
    // The lines that say why, from the failed command's newest logs.
    let mut tail = String::new();
    for (_, path) in files.iter().rev().take(4) {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let why: Vec<&str> = text
            .lines()
            .filter(|l| {
                [
                    "panic",
                    "fatal",
                    "FATAL",
                    "error",
                    "Error",
                    "denied",
                    "unsupported",
                ]
                .iter()
                .any(|k| l.contains(k))
            })
            .take(12)
            .collect();
        if !why.is_empty() {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            tail += &format!("\n--- {}:\n{}", name.unwrap_or_default(), why.join("\n"));
        }
    }
    let how = if matches!(out.status.code(), Some(124 | 137)) || out.status.code().is_none() {
        "did not finish within 60 s".to_string()
    } else {
        format!("failed ({})", out.status)
    };
    Err(format!(
        "runsc {what} {how}: {}{}{tail}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    ))
}

/// Removes sandbox `id`, whatever state it is in (bounded like every
/// `runsc` command; output discarded, so no pipe can keep it waiting).
fn remove(runsc: &Path, root: &Path, id: &str) {
    let _ = runsc_cmd(runsc, root)
        .args(["delete", "--force", id])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// An OCI bundle in `dir` whose root holds `program` as `/prog`.
fn gvisor_bundle(runsc: &Path, dir: &Path, program: &Path, logs: &Path) -> Result<String> {
    let rootfs = dir.join("rootfs");
    std::fs::create_dir_all(&rootfs).map_err(|e| e.to_string())?;
    std::fs::copy(program, rootfs.join("prog")).map_err(|e| e.to_string())?;
    checked(
        Command::new(runsc).arg("spec").current_dir(dir),
        "spec",
        logs,
    )?;
    let spec = std::fs::read_to_string(dir.join("config.json")).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("config.json"), gvisor_config(&spec)?).map_err(|e| e.to_string())?;
    Ok(dir.display().to_string())
}

/// The OCI configuration for a bundle running `/prog`: no terminal (it may
/// run detached), read-only root.
pub fn gvisor_config(spec: &str) -> Result<String> {
    let mut v: serde_json::Value =
        serde_json::from_str(spec).map_err(|e| format!("bad runsc spec output: {e}"))?;
    v["process"]["terminal"] = serde_json::Value::Bool(false);
    v["process"]["args"] = serde_json::json!(["/prog"]);
    v["root"]["readonly"] = serde_json::Value::Bool(true);
    serde_json::to_string_pretty(&v).map_err(|e| e.to_string())
}

/// gVisor (design D1 of `add-gvisor-comparison`): `sandbox.run_ms` (a new
/// sandbox running the cells' static `hello` to its exit) and
/// `snapshot.restore_ms` (restoring a checkpointed sandbox of `idle` until it
/// runs), each over `runs` runs after one warm-up.
pub fn gvisor(cells: &Path, runs: usize, scratch: &Path) -> Result<Vec<Metric>> {
    let runsc = runsc()?;
    let dir = scratch.join(format!("gvisor-{}", std::process::id()));
    let version = checked(Command::new(&runsc).arg("--version"), "--version", &dir)?;
    let root = dir.join("state");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let logs = dir.join("logs");
    std::fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
    // The whole comparison stays well inside the CI job's time limit.
    const BUDGET: Duration = Duration::from_secs(300);
    let started = Instant::now();
    let result = (|| {
        // A sandbox running `hello` to its exit.
        // Everything on disk first: once runsc has run as root, it may have
        // taken over the permissions of the directories it was given.
        let hello = gvisor_bundle(&runsc, &dir.join("hello"), &cells.join("hello"), &logs)?;
        let bundle_arg = gvisor_bundle(&runsc, &dir.join("idle"), &cells.join("idle"), &logs)?;
        let image = dir.join("image");
        std::fs::create_dir_all(&image).map_err(|e| e.to_string())?;
        let mut run = Vec::new();
        for i in 0..=runs {
            if started.elapsed() > BUDGET {
                return Err(format!("over {BUDGET:?} after {i} hello runs: {run:?} ms"));
            }
            let id = format!("rz-hello-{i}");
            let t0 = Instant::now();
            let out = checked(
                runsc_cmd(&runsc, &root).args(["run", "--bundle", &hello, &id]),
                "run (hello)",
                &logs,
            );
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            remove(&runsc, &root, &id);
            let out = out?;
            if !out.contains("hello from a cell") {
                return Err(format!("runsc run printed {out:?}"));
            }
            if i > 0 {
                run.push(ms);
            }
        }
        // Checkpoint a sandbox of `idle` once, then restore new ones from it.
        let base = checked(
            runsc_cmd(&runsc, &root).args(["run", "--detach", "--bundle", &bundle_arg, "rz-base"]),
            "run --detach (idle)",
            &logs,
        )
        .and_then(|_| {
            checked(
                runsc_cmd(&runsc, &root)
                    .arg("checkpoint")
                    .arg("--image-path")
                    .arg(&image)
                    .arg("rz-base"),
                "checkpoint",
                &logs,
            )
        });
        remove(&runsc, &root, "rz-base");
        base?;
        let mut restore = Vec::new();
        for i in 0..=runs {
            if started.elapsed() > BUDGET {
                return Err(format!(
                    "over {BUDGET:?} after {i} restores: {restore:?} ms"
                ));
            }
            let id = format!("rz-{i}");
            let t0 = Instant::now();
            let state = checked(
                runsc_cmd(&runsc, &root)
                    .arg("restore")
                    .arg("--detach")
                    .arg("--image-path")
                    .arg(&image)
                    .args(["--bundle", &bundle_arg, &id]),
                "restore",
                &logs,
            )
            .and_then(|_| {
                checked(
                    runsc_cmd(&runsc, &root).args(["state", &id]),
                    "state",
                    &logs,
                )
            });
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            remove(&runsc, &root, &id);
            let state = state?;
            if !state.contains("\"running\"") {
                return Err(format!("restored sandbox is not running: {state}"));
            }
            if i > 0 {
                restore.push(ms);
            }
        }
        Ok(vec![
            metric("sandbox.run_ms", "gvisor", "runsc", "ms", &mut run),
            metric("snapshot.restore_ms", "gvisor", "runsc", "ms", &mut restore),
        ])
    })();
    // The state directory is root-owned; leave it to the next run's clean-up.
    let _ = std::fs::remove_dir_all(&dir);
    report::text(&format!(
        "gvisor ({}): {}",
        version.lines().next().unwrap_or("runsc"),
        if result.is_ok() { "measured" } else { "failed" }
    ));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn criu_restore_time_is_the_last_log_timestamp() {
        let log = "(00.000010) Version: 3.19\n(00.004500) Restoring...\n\
            (00.012345) Restore finished successfully. Tasks resumed.\n\n";
        assert_eq!(criu_log_seconds(log), Some(0.012345));
        assert_eq!(criu_log_seconds("no timestamps"), None);
    }

    #[test]
    fn cpython_line_is_found_among_console_noise() {
        let line = "[  1.2] junk linux-cpython cold_us=31000.5 warm_us=2100.25\r\n";
        assert_eq!(parse_cpython_line(line), Some([31000.5, 2100.25]));
        assert_eq!(
            parse_cpython_line("linux-cpython cold_us=1 warm_us=failed"),
            None
        );
        assert_eq!(parse_cpython_line("linux-process fork_us=1"), None);
    }

    #[test]
    fn process_line_is_found_among_console_noise() {
        let console = "[    0.1] Run /init as init process\r\n\
            junk linux-process fork_us=1.5 fork_call_us=2 fork_touch_us=3.25 spawn_us=4\r\n";
        assert_eq!(parse_process_line(console), Some([1.5, 2.0, 3.25, 4.0]));
        assert_eq!(
            parse_process_line("linux-process fork_us=1 spawn_us=4"),
            None
        );
        let m = process_metrics("firecracker", &[[1.0, 2.0, 3.0, 4.0], [3.0, 2.0, 3.0, 4.0]]);
        let names: Vec<_> = m.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "process.fork_us",
                "process.fork_call_us",
                "process.fork_touch_us",
                "process.spawn_us"
            ]
        );
        assert!(
            m.iter()
                .all(|m| m.guest == "linux-process" && m.vmm.as_deref() == Some("firecracker"))
        );
    }

    #[test]
    fn gvisor_bundle_runs_its_program_without_a_terminal() {
        let spec = r#"{"ociVersion":"1.0.0","process":{"terminal":true,"args":["sh"],"cwd":"/"},
            "root":{"path":"rootfs","readonly":false}}"#;
        let v: serde_json::Value = serde_json::from_str(&gvisor_config(spec).unwrap()).unwrap();
        assert_eq!(v["process"]["terminal"], false);
        assert_eq!(v["process"]["args"], serde_json::json!(["/prog"]));
        assert_eq!(v["root"]["readonly"], true);
        assert_eq!(v["root"]["path"], "rootfs", "the rest is kept");
        assert!(gvisor_config("not json").is_err());
    }
}
