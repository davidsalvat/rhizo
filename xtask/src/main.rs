//! Developer tasks for Rhizo.
//!
//! Run `cargo xtask help` for the list of commands. Only the Rust toolchain
//! and QEMU (`qemu-system-x86_64`) are required; Firecracker, Cloud Hypervisor
//! and the reference guest are downloaded on demand.

mod bench;
mod refguest;
mod tools;
mod vmm;

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;
use vmm::{Accel, Guest, RunOptions, VmResult, VmRun, Vmm};

const KERNEL_TARGET: &str = "x86_64-unknown-none";
const KERNEL_PACKAGE: &str = "rhizo-kernel";
const DEFAULT_TEST_TIMEOUT_SECS: u64 = 30;
/// Timeout for the self-check that expects a hanging kernel.
const HANG_CHECK_TIMEOUT_SECS: u64 = 5;

const HELP: &str = "\
Usage: cargo xtask <command> [options]

Commands:
  build [--release]          Build the kernel image
  run [options]              Build and boot the kernel, console on stdout,
                             then report the time to ready
      --release              Optimized build
      --vmm <name>           qemu-microvm (default), qemu-q35, firecracker,
                             cloud-hypervisor
      --machine <microvm|q35>  Shorthand for the QEMU VMMs
      --accel <auto|kvm|tcg> QEMU acceleration (default auto: KVM if usable)
      --gdb                  QEMU only: wait for a debugger on localhost:1234
  test [--vmm <name>] [--accel <a>]
                             Host tests, boot check, in-kernel tests and
                             harness self-checks
  bench [options]            Release-build boot benchmarks on every VMM
      --runs <n>             Boots per VMM (default 20)
      --vmm <a,b,...>        Restrict the VMMs
      --env <name>           Environment label / baseline (ci, dedicated, local)
      --output <file>        Write results as JSON
      --from <file>          Use stored results instead of measuring
      --check                Fail if a metric regressed beyond its threshold
      --update-baseline      Store the results as the baseline for --env
      --readme               Regenerate the README benchmark section
      --no-reference         Skip the reference Linux guest
  ci                         Everything CI runs: fmt, clippy, test
  help                       Show this message

Environment:
  RHIZO_TEST_TIMEOUT         Seconds before a test VM is killed (default 30)
  RHIZO_QEMU                 QEMU binary (default qemu-system-x86_64)
  RHIZO_FIRECRACKER, RHIZO_CLOUD_HYPERVISOR, RHIZO_REFERENCE_KERNEL
                             Use local files instead of downloading
";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    let result = match args.first().map(String::as_str) {
        Some("build") => {
            build_kernel(Profile::from_args(rest), &[]).map(|path| println!("{}", path.display()))
        }
        Some("run") => RunArgs::parse(rest).and_then(|a| cmd_run(&a)),
        Some("test") => RunArgs::parse(rest).and_then(|a| cmd_test(&a)),
        Some("bench") => bench::Options::parse(rest).and_then(|o| bench::cmd(&o)),
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

pub(crate) type Result<T = ()> = std::result::Result<T, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Profile {
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

/// Options shared by `run` and `test`.
#[derive(Debug)]
struct RunArgs {
    profile: Profile,
    vmm: Vmm,
    accel: Accel,
    gdb: bool,
}

impl RunArgs {
    fn parse(args: &[String]) -> Result<Self> {
        let mut parsed = Self {
            profile: Profile::from_args(args),
            vmm: Vmm::QemuMicrovm,
            accel: Accel::Auto,
            gdb: false,
        };
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let mut value = |name: &str| {
                it.next()
                    .cloned()
                    .ok_or_else(|| format!("{name} needs a value"))
            };
            match arg.as_str() {
                "--release" => {}
                "--gdb" => parsed.gdb = true,
                "--vmm" => parsed.vmm = Vmm::parse(&value("--vmm")?)?,
                "--machine" => {
                    parsed.vmm = match value("--machine")?.as_str() {
                        "microvm" => Vmm::QemuMicrovm,
                        "q35" => Vmm::QemuQ35,
                        other => return Err(format!("unknown machine `{other}` (microvm or q35)")),
                    }
                }
                "--accel" => parsed.accel = Accel::parse(&value("--accel")?)?,
                other => return Err(format!("unknown option `{other}`")),
            }
        }
        if parsed.gdb && parsed.vmm.requires_kvm() {
            return Err("--gdb is only supported on the QEMU VMMs".into());
        }
        Ok(parsed)
    }

    fn run_options(&self, timeout: Duration) -> RunOptions {
        RunOptions {
            stop_at_ready: false,
            timeout,
            echo: true,
            accel: self.accel,
            gdb: self.gdb,
        }
    }
}

fn cmd_run(args: &RunArgs) -> Result {
    let kernel = build_kernel(args.profile, &[])?;
    if args.gdb {
        println!("QEMU is waiting for gdb on localhost:1234");
    }
    let timeout = if args.gdb {
        Duration::from_secs(24 * 3600)
    } else {
        test_timeout()?
    };
    let run = vmm::run(args.vmm, &Guest::Rhizo(kernel), args.run_options(timeout))?;
    report_timing(args.vmm, &run);
    match run.result {
        VmResult::Passed => Ok(()),
        other => Err(format!(
            "kernel run on {} ended with {other:?}",
            args.vmm.id()
        )),
    }
}

/// Prints the host-measured time to ready and the kernel's own cycle count.
fn report_timing(vmm: Vmm, run: &VmRun) {
    match run.time_to_ready {
        Some(t) => println!(
            "[xtask] {}: time to ready {:.2} ms (host), {} kernel cycles",
            vmm.id(),
            t.as_secs_f64() * 1000.0,
            run.ready_cycles
                .map_or_else(|| "unknown".to_string(), |c| c.to_string())
        ),
        None => println!("[xtask] {}: the ready marker never arrived", vmm.id()),
    }
}

fn cmd_test(args: &RunArgs) -> Result {
    step("host tests");
    cargo(&["test", "--workspace", "--exclude", KERNEL_PACKAGE])?;

    let timeout = test_timeout()?;
    let opts = args.run_options(timeout);

    step(&format!("boot check on {}", args.vmm.id()));
    let kernel = build_kernel(Profile::Debug, &[])?;
    let run = vmm::run(args.vmm, &Guest::Rhizo(kernel), opts)?;
    report_timing(args.vmm, &run);
    expect(
        run.result == VmResult::Passed,
        "boot must exit successfully",
        &run,
    )?;
    for needle in [
        "Rhizo ",
        "\nmemory: ",
        "\nrhizo-ready cycles=",
        "boot complete",
    ] {
        expect(
            run.output.contains(needle),
            &format!("boot output must contain `{}`", needle.trim()),
            &run,
        )?;
    }

    step("in-kernel tests");
    let kernel = build_kernel(Profile::Debug, &["ktest"])?;
    let run = vmm::run(args.vmm, &Guest::Rhizo(kernel), opts)?;
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
        let run = vmm::run(
            args.vmm,
            &Guest::Rhizo(kernel),
            RunOptions {
                timeout: limit,
                ..opts
            },
        )?;
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

    cmd_test(&RunArgs::parse(&[])?)?;

    step("boot check on qemu-q35");
    let kernel = build_kernel(Profile::Debug, &[])?;
    let q35 = RunArgs::parse(&["--machine".into(), "q35".into()])?;
    let run = vmm::run(
        Vmm::QemuQ35,
        &Guest::Rhizo(kernel),
        q35.run_options(test_timeout()?),
    )?;
    report_timing(Vmm::QemuQ35, &run);
    expect(
        run.result == VmResult::Passed,
        "q35 boot must exit successfully",
        &run,
    )
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

pub(crate) fn build_kernel(profile: Profile, features: &[&str]) -> Result<PathBuf> {
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

pub(crate) fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the workspace")
        .to_path_buf()
}

pub(crate) fn target_dir() -> PathBuf {
    env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target"))
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
    fn run_args_parse_vmm_machine_and_accel() {
        let parse =
            |a: &[&str]| RunArgs::parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let a = parse(&[]).unwrap();
        assert_eq!(
            (a.vmm, a.accel, a.profile),
            (Vmm::QemuMicrovm, Accel::Auto, Profile::Debug)
        );
        assert_eq!(parse(&["--machine", "q35"]).unwrap().vmm, Vmm::QemuQ35);
        assert_eq!(
            parse(&["--vmm", "firecracker"]).unwrap().vmm,
            Vmm::Firecracker
        );
        assert_eq!(parse(&["--accel", "tcg"]).unwrap().accel, Accel::Tcg);
        assert!(parse(&["--machine", "pc"]).is_err());
        assert!(parse(&["--vmm", "firecracker", "--gdb"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
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
