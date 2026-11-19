//! Developer tasks for Rhizo.
//!
//! Run `cargo xtask help` for the list of commands. Only the Rust toolchain
//! and QEMU (`qemu-system-x86_64`) are required.

use std::env;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const KERNEL_TARGET: &str = "x86_64-unknown-none";
const KERNEL_PACKAGE: &str = "rhizo-kernel";
const DEFAULT_TEST_TIMEOUT_SECS: u64 = 30;
/// Timeout for the self-check that expects a hanging kernel.
const HANG_CHECK_TIMEOUT_SECS: u64 = 5;

/// QEMU exit status produced by the kernel writing 0x10 to `isa-debug-exit`.
const QEMU_SUCCESS: i32 = (0x10 << 1) | 1;
/// QEMU exit status produced by the kernel writing 0x11 to `isa-debug-exit`.
const QEMU_FAILURE: i32 = (0x11 << 1) | 1;

const HELP: &str = "\
Usage: cargo xtask <command> [options]

Commands:
  build [--release]          Build the kernel image
  run [--release] [--gdb]    Build and boot the kernel in QEMU (serial on stdout)
                             --gdb waits for a debugger on localhost:1234
  test                       Host tests, boot check, in-kernel tests in QEMU,
                             and harness self-checks
  ci                         Everything CI runs: fmt, clippy, test
  help                       Show this message

Environment:
  RHIZO_TEST_TIMEOUT         Seconds before a test VM is killed (default 30)
  RHIZO_QEMU                 QEMU binary (default qemu-system-x86_64)
";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("build") => {
            let profile = Profile::from_args(&args[1..]);
            build_kernel(profile, &[]).map(|path| println!("{}", path.display()))
        }
        Some("run") => {
            let profile = Profile::from_args(&args[1..]);
            let gdb = args.iter().any(|a| a == "--gdb");
            cmd_run(profile, gdb)
        }
        Some("test") => cmd_test(),
        Some("ci") => cmd_ci(),
        Some("help") | Some("--help") | Some("-h") | None => {
            print!("{HELP}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{HELP}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

type Result<T = ()> = std::result::Result<T, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Profile {
    Debug,
    Release,
}

impl Profile {
    fn from_args(args: &[String]) -> Self {
        if args.iter().any(|a| a == "--release") {
            Profile::Release
        } else {
            Profile::Debug
        }
    }

    fn dir(self) -> &'static str {
        match self {
            Profile::Debug => "debug",
            Profile::Release => "release",
        }
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn cmd_run(profile: Profile, gdb: bool) -> Result {
    let kernel = build_kernel(profile, &[])?;
    let mut qemu = qemu_command(&kernel);
    if gdb {
        qemu.args(["-s", "-S"]);
        println!("QEMU is waiting for gdb on localhost:1234");
    }
    let status = qemu
        .stdin(Stdio::inherit())
        .status()
        .map_err(|e| format!("failed to start QEMU: {e}"))?;
    match interpret_exit(status.code()) {
        VmResult::Passed => Ok(()),
        other => Err(format!("kernel run ended with {other:?}")),
    }
}

fn cmd_test() -> Result {
    step("host tests");
    cargo(&["test", "--workspace", "--exclude", KERNEL_PACKAGE])?;

    let timeout = test_timeout()?;

    step("boot check");
    let kernel = build_kernel(Profile::Debug, &[])?;
    let run = run_vm(&kernel, timeout)?;
    expect(
        run.result == VmResult::Passed,
        "boot must exit successfully",
        &run,
    )?;
    expect(
        run.output.contains("Rhizo ")
            && run.output.contains("\nmemory: ")
            && run.output.contains("boot complete"),
        "boot must print the banner, the memory line and `boot complete`",
        &run,
    )?;

    step("in-kernel tests");
    let kernel = build_kernel(Profile::Debug, &["ktest"])?;
    let run = run_vm(&kernel, timeout)?;
    expect(
        run.result == VmResult::Passed,
        "in-kernel tests must pass",
        &run,
    )?;
    expect(
        run.output.contains("test result: ok."),
        "in-kernel tests must report their result",
        &run,
    )?;

    for check in SELF_CHECKS {
        step(&format!("self-check: {}", check.description));
        let kernel = build_kernel(Profile::Debug, &[check.feature])?;
        let limit = check
            .timeout_secs
            .map(Duration::from_secs)
            .unwrap_or(timeout);
        let run = run_vm(&kernel, limit)?;
        expect(
            run.result == check.expected,
            &format!("{} must end with {:?}", check.feature, check.expected),
            &run,
        )?;
        for needle in check.output {
            expect(
                run.output.contains(needle),
                &format!("{} output must contain `{needle}`", check.feature),
                &run,
            )?;
        }
    }

    println!("\nall checks passed");
    Ok(())
}

fn cmd_ci() -> Result {
    step("format");
    cargo(&["fmt", "--all", "--check"])?;

    step("clippy (host crates)");
    cargo(&[
        "clippy",
        "--workspace",
        "--exclude",
        KERNEL_PACKAGE,
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ])?;

    let features = [""]
        .into_iter()
        .chain(["ktest"])
        .chain(SELF_CHECKS.iter().map(|c| c.feature));
    for features in features {
        step(&format!("clippy (kernel, features: `{features}`)"));
        let mut args = vec!["clippy", "-p", KERNEL_PACKAGE, "--target", KERNEL_TARGET];
        if !features.is_empty() {
            args.extend(["--features", features]);
        }
        args.extend(["--", "-D", "warnings"]);
        cargo(&args)?;
    }

    cmd_test()
}

/// A kernel build that must fail in a specific, detectable way.
struct SelfCheck {
    feature: &'static str,
    description: &'static str,
    expected: VmResult,
    /// Overrides the test timeout (for checks that expect a timeout).
    timeout_secs: Option<u64>,
    output: &'static [&'static str],
}

const SELF_CHECKS: &[SelfCheck] = &[
    SelfCheck {
        feature: "ktest-should-panic",
        description: "a panicking test fails the run",
        expected: VmResult::Failed,
        timeout_secs: None,
        output: &[
            "test deliberate_panic ... ",
            "PANIC at kernel/src/tests.rs:",
            "deliberate panic",
        ],
    },
    SelfCheck {
        feature: "ktest-hang",
        description: "a hanging test times out",
        expected: VmResult::TimedOut,
        timeout_secs: Some(HANG_CHECK_TIMEOUT_SECS),
        output: &["test hang_forever ... "],
    },
    SelfCheck {
        feature: "ktest-page-fault",
        description: "a null write is reported as a page fault",
        expected: VmResult::Failed,
        timeout_secs: None,
        output: &[
            "EXCEPTION: page fault (vector 14",
            "address 0x0: write, page not present, kernel mode",
        ],
    },
    SelfCheck {
        feature: "ktest-stack-overflow",
        description: "stack exhaustion is reported as a double fault",
        expected: VmResult::Failed,
        timeout_secs: None,
        output: &["EXCEPTION: double fault (vector 8"],
    },
    SelfCheck {
        feature: "ktest-invalid-opcode",
        description: "an invalid instruction is reported",
        expected: VmResult::Failed,
        timeout_secs: None,
        output: &["EXCEPTION: invalid opcode (vector 6"],
    },
    SelfCheck {
        feature: "ktest-oom",
        description: "an impossible allocation panics with its size",
        expected: VmResult::Failed,
        timeout_secs: None,
        output: &[
            "PANIC at",
            "memory allocation of 1099511627776 bytes failed",
        ],
    },
];

// ---------------------------------------------------------------------------
// Building
// ---------------------------------------------------------------------------

fn build_kernel(profile: Profile, features: &[&str]) -> Result<PathBuf> {
    let mut args = vec!["build", "-p", KERNEL_PACKAGE, "--target", KERNEL_TARGET];
    if profile == Profile::Release {
        args.push("--release");
    }
    let joined = features.join(",");
    if !features.is_empty() {
        args.extend(["--features", &joined]);
    }
    cargo(&args)?;
    Ok(target_dir()
        .join(KERNEL_TARGET)
        .join(profile.dir())
        .join(KERNEL_PACKAGE))
}

fn cargo(args: &[&str]) -> Result {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(&cargo)
        .args(args)
        .current_dir(workspace_root())
        .status()
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`cargo {}` failed", args.join(" ")))
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the workspace")
        .to_path_buf()
}

fn target_dir() -> PathBuf {
    env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target"))
}

// ---------------------------------------------------------------------------
// Running QEMU
// ---------------------------------------------------------------------------

/// How a VM run ended, as seen by the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VmResult {
    /// The kernel reported success through the exit device.
    Passed,
    /// The kernel reported failure through the exit device.
    Failed,
    /// QEMU ended any other way: crash, triple fault (with -no-reboot), error.
    Abnormal(Option<i32>),
    /// The VM did not exit before the time limit and was killed.
    TimedOut,
}

/// Maps QEMU's process exit code to a [`VmResult`] (design decision D6).
fn interpret_exit(code: Option<i32>) -> VmResult {
    match code {
        Some(QEMU_SUCCESS) => VmResult::Passed,
        Some(QEMU_FAILURE) => VmResult::Failed,
        other => VmResult::Abnormal(other),
    }
}

#[derive(Debug)]
struct VmRun {
    result: VmResult,
    output: String,
}

fn qemu_command(kernel: &Path) -> Command {
    let qemu = env::var("RHIZO_QEMU").unwrap_or_else(|_| "qemu-system-x86_64".into());
    let mut cmd = Command::new(qemu);
    cmd.args(["-machine", "q35", "-cpu", "max", "-m", "128M"])
        .args(["-display", "none", "-serial", "stdio", "-no-reboot"])
        .args(["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"])
        .args(["-accel", if kvm_available() { "kvm" } else { "tcg" }])
        .arg("-kernel")
        .arg(kernel);
    cmd
}

fn kvm_available() -> bool {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
        .is_ok()
}

/// Boots `kernel`, streams its serial output to stdout and captures it.
fn run_vm(kernel: &Path, timeout: Duration) -> Result<VmRun> {
    let mut child = qemu_command(kernel)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to start QEMU (is it installed?): {e}"))?;

    let captured = Arc::new(Mutex::new(Vec::new()));
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let sink = Arc::clone(&captured);
    let reader = thread::spawn(move || {
        let mut buf = [0u8; 1024];
        while let Ok(n) = stdout.read(&mut buf) {
            if n == 0 {
                break;
            }
            let chunk = &buf[..n];
            let _ = io::stdout().write_all(chunk);
            let _ = io::stdout().flush();
            sink.lock().expect("capture lock").extend_from_slice(chunk);
        }
    });

    let started = Instant::now();
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => break interpret_exit(status.code()),
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break VmResult::TimedOut;
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("failed to wait for QEMU: {e}")),
        }
    };
    let _ = reader.join();

    if result == VmResult::TimedOut {
        println!("\n[xtask] VM killed after {}s timeout", timeout.as_secs());
    }
    let output = String::from_utf8_lossy(&captured.lock().expect("capture lock")).into_owned();
    Ok(VmRun { result, output })
}

fn test_timeout() -> Result<Duration> {
    match env::var("RHIZO_TEST_TIMEOUT") {
        Ok(value) => parse_timeout(&value),
        Err(_) => Ok(Duration::from_secs(DEFAULT_TEST_TIMEOUT_SECS)),
    }
}

fn parse_timeout(value: &str) -> Result<Duration> {
    match value.trim().parse::<u64>() {
        Ok(secs) if secs > 0 => Ok(Duration::from_secs(secs)),
        _ => Err(format!(
            "RHIZO_TEST_TIMEOUT must be a positive number of seconds, got `{value}`"
        )),
    }
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

fn step(name: &str) {
    println!("\n==> {name}");
}

fn expect(condition: bool, what: &str, run: &VmRun) -> Result {
    if condition {
        Ok(())
    } else {
        Err(format!("{what} (VM result: {:?})", run.result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_map_to_results() {
        assert_eq!(interpret_exit(Some(33)), VmResult::Passed);
        assert_eq!(interpret_exit(Some(35)), VmResult::Failed);
        assert_eq!(interpret_exit(Some(0)), VmResult::Abnormal(Some(0)));
        assert_eq!(interpret_exit(Some(1)), VmResult::Abnormal(Some(1)));
        assert_eq!(interpret_exit(None), VmResult::Abnormal(None));
    }

    #[test]
    fn timeout_must_be_positive_seconds() {
        assert_eq!(parse_timeout("12"), Ok(Duration::from_secs(12)));
        assert_eq!(parse_timeout(" 3 "), Ok(Duration::from_secs(3)));
        assert!(parse_timeout("0").is_err());
        assert!(parse_timeout("-1").is_err());
        assert!(parse_timeout("soon").is_err());
    }

    #[test]
    fn profile_is_release_only_when_asked() {
        assert_eq!(Profile::from_args(&[]), Profile::Debug);
        assert_eq!(
            Profile::from_args(&["--gdb".into(), "--release".into()]),
            Profile::Release
        );
    }
}
