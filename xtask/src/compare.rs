//! Comparisons with existing isolation mechanisms, measured on the same
//! machine as the cells (spec `comparisons`):
//!
//! - Linux processes: `fork` of a process with 50 MiB written, and spawning
//!   the same static `hello` program the cell benchmark runs.
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

/// `fork_us` and `spawn_us` of Linux processes on this host, running the
/// cell `hello` (from `cells`) natively. Needs a Linux host of the target
/// architecture.
pub fn linux_process(cells: &Path, scratch: &Path) -> Result<Vec<Metric>> {
    if crate::differential::unavailable_reason().is_some() {
        return Err("the host cannot run the cells natively".into());
    }
    let source = workspace_root().join("bench/host/linux_process.c");
    let binary = scratch.join("linux_process");
    let status = Command::new("cc")
        .args(["-O2", "-o"])
        .arg(&binary)
        .arg(&source)
        .status()
        .map_err(|e| format!("cannot run cc: {e}"))?;
    if !status.success() {
        return Err(format!("cannot compile {}", source.display()));
    }
    let (mut fork, mut spawn) = (Vec::new(), Vec::new());
    for _ in 0..3 {
        let out = Command::new(&binary)
            .arg(cells.join("hello"))
            .stderr(Stdio::inherit())
            .output()
            .map_err(|e| format!("cannot run {}: {e}", binary.display()))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text
            .lines()
            .find(|l| l.starts_with("linux-process "))
            .ok_or("linux_process printed no result line")?;
        let field = |key: &str| -> Option<f64> {
            line.split_whitespace()
                .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?
                .parse()
                .ok()
        };
        fork.push(field("fork_us").ok_or("no fork_us")?);
        spawn.push(field("spawn_us").ok_or("no spawn_us")?);
    }
    Ok(vec![
        metric("process.fork_us", "linux-process", "host", "us", &mut fork),
        metric(
            "process.spawn_us",
            "linux-process",
            "host",
            "us",
            &mut spawn,
        ),
    ])
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

/// The `runsc` binary: `RHIZO_RUNSC`, or `runsc` on `PATH`.
fn runsc() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("RHIZO_RUNSC") {
        return Ok(PathBuf::from(path));
    }
    let out = Command::new("sh")
        .args(["-c", "command -v runsc"])
        .output()
        .map_err(|e| format!("cannot look for runsc: {e}"))?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        return Err("gVisor's runsc is not installed (CI installs it; see bench/README.md)".into());
    }
    Ok(PathBuf::from(path))
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
