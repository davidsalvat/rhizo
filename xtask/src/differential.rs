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
    AGENT_TRANSCRIPT, AGENT_TURNS, CPYTHON_ARGV, CPYTHON_STDOUT, DYNAMIC, Ending, NODE_ARGV,
    NODE_MODEL_LINE, NODE_STDOUT, PROGRAMS, PYTHON_COMMAND, PYTHON_STDOUT,
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
    check_cpython(&scratch)?;
    check_dynamic()?;
    check_node()
}

/// Node.js, when packed (`RHIZO_NODE`): from its tree laid out as in a cell,
/// with an empty environment, it prints [`NODE_STDOUT`], then, given the
/// URL of the host's fake model, [`NODE_MODEL_LINE`], and nothing on
/// standard error (change `add-node-workload`).
fn check_node() -> Result {
    let root = &crate::dynamic::roots()[2];
    let node = root.join(crate::dynamic::NODE);
    if !node.exists() {
        crate::report::text("[xtask] Node.js not packed (RHIZO_NODE unset): skipped");
        return Ok(());
    }
    let url = format!(
        "http://127.0.0.1:{}",
        crate::hostnet::llm_server("127.0.0.1", 0)?
    );
    let script = root.join(NODE_ARGV[2].trim_start_matches('/'));
    for (url, stdout) in [
        (None, NODE_STDOUT.to_string()),
        (Some(url), format!("{NODE_STDOUT}{NODE_MODEL_LINE}")),
    ] {
        let out = Command::new(&node)
            .arg(NODE_ARGV[1])
            .arg(&script)
            .args(&url)
            .env_clear()
            .current_dir(root)
            .output()
            .map_err(|e| format!("cannot run {}: {e}", node.display()))?;
        let text = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() || text != stdout || !out.stderr.is_empty() {
            return Err(
                Failure::new("differential", "Node.js behaves differently on Linux")
                    .location("cells/node/demo.js")
                    .expected(format!(
                        "exit 0, stdout {stdout:?} (cells/expected.rs), no stderr"
                    ))
                    .actual(format!(
                        "{:?}, stdout {text:?}, stderr {:?}",
                        out.status,
                        String::from_utf8_lossy(&out.stderr)
                    ))
                    .hint("Linux is the reference: fix the script or the expectation")
                    .reproduce(format!(
                        "env -i {} {} {} {}",
                        node.display(),
                        NODE_ARGV[1],
                        script.display(),
                        url.unwrap_or_default()
                    ))
                    .into_error(),
            );
        }
    }
    Ok(())
}

/// The dynamically linked programs packed for cells (`crate::dynamic`), run
/// natively from the top of their tree with an empty environment, as in a
/// cell: each prints what `cells/expected.rs` says and exits with 0.
fn check_dynamic() -> Result {
    let roots = crate::dynamic::roots();
    for &(path, argv, stdout) in DYNAMIC {
        let Some(root) = roots.iter().find(|r| r.join(path).exists()) else {
            crate::report::text(&format!("[xtask] {path} not packed: skipped"));
            continue;
        };
        let program = root.join(path);
        let out = Command::new(&program)
            .arg0(argv[0])
            .args(&argv[1..])
            .env_clear()
            .current_dir(root)
            .output()
            .map_err(|e| format!("cannot run {}: {e}", program.display()))?;
        let text = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() || text != stdout {
            return Err(Failure::new(
                "differential",
                format!("`{path}` behaves differently on Linux"),
            )
            .location("cells/expected.rs (DYNAMIC)")
            .expected(format!("exit 0, stdout {stdout:?}"))
            .actual(format!(
                "{:?}, stdout {text:?}, stderr {:?}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            ))
            .hint("Linux is the reference: fix the program or the expectation")
            .reproduce(format!(
                "cd {} && env -i {path} {}",
                root.display(),
                argv[1..].join(" ")
            ))
            .into_error());
        }
    }
    Ok(())
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
    let port = crate::hostnet::llm_server("127.0.0.1", 0)?;
    let out = run_agent(python, root, scratch, port)?;
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
    // A real model may answer without calling the tool, in chunks.
    let out = run_agent(python, root, scratch, chatty_model()?)?;
    let text = String::from_utf8_lossy(&out.stdout);
    let turns = text.lines().filter(|l| l.contains("turn")).count();
    let replies = text
        .lines()
        .filter(|l| l.contains("reply chars=12"))
        .count();
    if !out.status.success() || turns != AGENT_TURNS + 3 || replies != turns {
        return Err(Failure::new(
            "differential",
            "the CPython agent fails with a model that does not call its tool",
        )
        .location("cells/python/agent_loop.py")
        .expected(format!(
            "exit 0, {} turns each answered `reply chars=12`",
            AGENT_TURNS + 3
        ))
        .actual(format!(
            "{:?}, stdout {text:?}, stderr {:?}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ))
        .hint("the agent must take a text answer as a turn and ask again")
        .reproduce("cargo xtask check")
        .into_error());
    }
    Ok(())
}

/// Runs the agent in `linux` mode for [`AGENT_TURNS`] turns against a
/// model on 127.0.0.1:`port`, in an empty `scratch` directory.
fn run_agent(
    python: &Path,
    root: &Path,
    scratch: &Path,
    port: u16,
) -> Result<std::process::Output> {
    let _ = std::fs::remove_dir_all(scratch);
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    let out = Command::new(python)
        .arg(root.join("app/agent_loop.py"))
        .arg(format!("http://127.0.0.1:{port}"))
        .args(["fake", "linux", &AGENT_TURNS.to_string()])
        .env_clear()
        .current_dir(scratch)
        .output()
        .map_err(|e| format!("cannot run {}: {e}", python.display()));
    let _ = std::fs::remove_dir_all(scratch);
    out
}

/// The port of a model on 127.0.0.1 that answers every request with text
/// and no tool call, in two chunks (`Transfer-Encoding: chunked`), as a
/// small real model may.
fn chatty_model() -> Result<u16> {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream);
                loop {
                    let mut length = 0;
                    let mut line = String::new();
                    while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line != "\r\n" {
                        if let Some((name, value)) = line.split_once(':')
                            && name.eq_ignore_ascii_case("content-length")
                        {
                            length = value.trim().parse().unwrap_or(0);
                        }
                        line.clear();
                    }
                    let mut body = vec![0; length];
                    if line.is_empty() || reader.read_exact(&mut body).is_err() {
                        return;
                    }
                    let (a, b) = (
                        r#"{"choices": [{"message": {"#,
                        r#""content": "no tool, sir"}}]}"#,
                    );
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                         {:x}\r\n{a}\r\n{:x}\r\n{b}\r\n0\r\n\r\n",
                        a.len(),
                        b.len()
                    );
                    if reader.get_mut().write_all(reply.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    Ok(port)
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
