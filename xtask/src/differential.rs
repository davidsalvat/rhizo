//! Differential check: the test cells are static Linux executables, so on a
//! Linux host of the same architecture they run natively. Their output and
//! ending must match `cells/expected.rs`, the expectations the in-kernel
//! tests hold Rhizo to (spec `linux-abi`: same binary, same behavior).

use crate::Result;
use crate::report::Failure;
use std::path::Path;
use std::process::Command;

mod expected {
    include!("../../cells/expected.rs");
}
use expected::{
    AGENT_TRANSCRIPT, AGENT_TURNS, CPYTHON_ARGV, CPYTHON_STDOUT, Ending, PROGRAMS, PYTHON_COMMAND,
    PYTHON_STDOUT,
};

/// Why the check cannot run on this host, if it cannot.
pub fn unavailable_reason() -> Option<String> {
    let host = std::env::consts::ARCH;
    let target = crate::arch::current().id();
    if std::env::consts::OS != "linux" {
        return Some("the host is not Linux".into());
    }
    (host != target).then(|| format!("the host is {host}, the cells are {target}"))
}

/// How a native run ended, in the same terms as the kernel's.
fn ending(status: std::process::ExitStatus) -> Option<Ending> {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => Some(Ending::Exit(code)),
        (None, Some(11)) => Some(Ending::Fault), // SIGSEGV
        _ => None,
    }
}

/// Runs every test cell in `dir` natively, each in a new empty directory
/// (a cell starts with an empty file system), and compares with the
/// expectations.
pub fn check(dir: &Path) -> Result {
    let scratch = crate::target_dir().join(format!("differential-{}", std::process::id()));
    for &(name, stdout, expected) in PROGRAMS {
        let path = dir.join(name);
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch)
            .map_err(|e| format!("cannot create {}: {e}", scratch.display()))?;
        let out = Command::new(&path)
            .arg0(name)
            .current_dir(&scratch)
            .output()
            .map_err(|e| format!("cannot run {}: {e}", path.display()))?;
        let _ = std::fs::remove_dir_all(&scratch);
        let got_text = String::from_utf8_lossy(&out.stdout);
        let got = ending(out.status);
        if got != Some(expected) || got_text != stdout {
            let reproduce = path.display().to_string();
            return Err(Failure::new(
                "differential",
                format!("`{name}` behaves differently on Linux"),
            )
            .location(format!("cells/src/bin/{name}.rs"))
            .expected(format!(
                "{expected:?}, stdout {stdout:?} (cells/expected.rs)"
            ))
            .actual(format!("{:?}, stdout {got_text:?}", out.status))
            .hint("Linux is the reference: fix the cell or the expectation, then make Rhizo match")
            .reproduce(&reproduce)
            .into_error());
        }
    }
    check_python(dir, &scratch)?;
    check_cpython(&scratch)
}

/// CPython, when built: its files laid out on the host as in a cell
/// (`usr/bin/python3`, `usr/lib/python3.13`, `app`), run with an empty environment
/// (a cell has none) from an empty directory, it prints [`CPYTHON_STDOUT`]
/// and nothing on standard error, as it must in a cell.
fn check_cpython(scratch: &Path) -> Result {
    let root = crate::cpython::root();
    let python = root.join("usr/bin/python3");
    if !python.exists() {
        crate::report::text("[xtask] CPython not built (RHIZO_CPYTHON_SRC unset): skipped");
        return Ok(());
    }
    let _ = std::fs::remove_dir_all(scratch);
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    let script = root.join(CPYTHON_ARGV[1].trim_start_matches('/'));
    let out = Command::new(&python)
        .arg(&script)
        .env_clear()
        .current_dir(scratch)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", python.display()))?;
    let _ = std::fs::remove_dir_all(scratch);
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || text != CPYTHON_STDOUT || !out.stderr.is_empty() {
        return Err(
            Failure::new("differential", "CPython behaves differently on Linux")
                .location("cells/python/agent_demo.py")
                .expected(format!(
                    "exit 0, stdout {CPYTHON_STDOUT:?} (cells/expected.rs), no stderr"
                ))
                .actual(format!(
                    "{:?}, stdout {text:?}, stderr {:?}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr)
                ))
                .hint("Linux is the reference: fix the script or the expectation")
                .reproduce(format!("env -i {} {}", python.display(), script.display()))
                .into_error(),
        );
    }
    check_agent(&python, &root, scratch)
}

/// The CPython agent as a Linux process (`linux` mode, its branches made
/// with `fork`) against the host's fake model prints [`AGENT_TRANSCRIPT`],
/// the transcript the cell must print (change `add-agent-workload`).
fn check_agent(python: &Path, root: &Path, scratch: &Path) -> Result {
    let port = crate::hostnet::llm_server("127.0.0.1", 0)?.to_string();
    let _ = std::fs::remove_dir_all(scratch);
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    let script = root.join("app/agent_loop.py");
    let turns = AGENT_TURNS.to_string();
    let args = [
        script.display().to_string(),
        "127.0.0.1".into(),
        port,
        "linux".into(),
        turns,
    ];
    let out = Command::new(python)
        .args(&args)
        .env_clear()
        .current_dir(scratch)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", python.display()))?;
    let _ = std::fs::remove_dir_all(scratch);
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || text != AGENT_TRANSCRIPT || !out.stderr.is_empty() {
        return Err(Failure::new(
            "differential",
            "the CPython agent behaves differently on Linux",
        )
        .location("cells/python/agent_loop.py")
        .expected(format!(
            "exit 0, stdout {AGENT_TRANSCRIPT:?} (cells/expected.rs), no stderr"
        ))
        .actual(format!(
            "{:?}, stdout {text:?}, stderr {:?}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ))
        .hint("Linux is the reference: fix the agent, the fake model or the expectation")
        .reproduce("cargo xtask check (the model server runs inside xtask)")
        .into_error());
    }
    Ok(())
}

/// MicroPython, when built (`micropython` in `dir`): run natively in an
/// empty directory holding `lib/rhizo_demo.py`, it prints
/// [`PYTHON_STDOUT`], as it must in a cell.
fn check_python(dir: &Path, scratch: &Path) -> Result {
    let path = dir.join("micropython");
    if !path.exists() {
        crate::report::text("[xtask] MicroPython not built (RHIZO_MICROPYTHON_SRC unset): skipped");
        return Ok(());
    }
    let _ = std::fs::remove_dir_all(scratch);
    let lib = scratch.join("lib");
    std::fs::create_dir_all(&lib).map_err(|e| format!("cannot create {}: {e}", lib.display()))?;
    let module = crate::workspace_root().join("cells/python/rhizo_demo.py");
    std::fs::copy(&module, lib.join("rhizo_demo.py")).map_err(|e| e.to_string())?;
    let out = Command::new(&path)
        .args(["-c", PYTHON_COMMAND])
        .current_dir(scratch)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", path.display()))?;
    let _ = std::fs::remove_dir_all(scratch);
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || text != PYTHON_STDOUT {
        return Err(
            Failure::new("differential", "MicroPython behaves differently on Linux")
                .location("cells/python/rhizo_demo.py")
                .expected(format!(
                    "exit 0, stdout {PYTHON_STDOUT:?} (cells/expected.rs)"
                ))
                .actual(format!(
                    "{:?}, stdout {text:?}, stderr {:?}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr)
                ))
                .hint("Linux is the reference: fix the module or the expectation")
                .reproduce(format!(
                    "cd <dir with lib/rhizo_demo.py> && {} -c \"{PYTHON_COMMAND}\"",
                    path.display()
                ))
                .into_error(),
        );
    }
    Ok(())
}

trait Arg0 {
    fn arg0(&mut self, name: &str) -> &mut Self;
}

impl Arg0 for Command {
    fn arg0(&mut self, name: &str) -> &mut Self {
        std::os::unix::process::CommandExt::arg0(self, name)
    }
}
