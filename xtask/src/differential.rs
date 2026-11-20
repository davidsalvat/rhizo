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
use expected::{Ending, PROGRAMS, PYTHON_COMMAND, PYTHON_STDOUT};

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
    check_python(dir, &scratch)
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
