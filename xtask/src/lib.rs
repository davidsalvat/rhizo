//! Developer tasks for Rhizo.
//!
//! Run `cargo xtask help` for the list of commands. Only the Rust toolchain
//! and QEMU (`qemu-system-x86_64`) are required; Firecracker, Cloud Hypervisor
//! and the reference guest are downloaded on demand.
//!
//! The library holds everything; the `xtask` binary only calls [`main`].
//! The `rhizo` command-line tool (`cli/`) uses the library too: building
//! the kernel, launching VMMs and the host's network side are the same code
//! for both (design D5 of `add-control-plane`).

pub mod arch;
mod bench;
mod compare;
pub mod control;
mod cpython;
mod demo;
mod deps;
mod differential;
mod dynamic;
mod packages;
pub mod hostnet;
mod profile;
mod refguest;
pub mod report;
pub mod tools;
pub mod vmm;
mod workloads;

use report::Failure;
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};
use vmm::{Accel, Guest, RunOptions, VmResult, VmRun, Vmm};

const KERNEL_PACKAGE: &str = "rhizo-kernel";
/// Test programs run as cells (bare-metal targets only).
const CELLS_PACKAGE: &str = "rhizo-cells";
const DEFAULT_TEST_TIMEOUT_SECS: u64 = 30;
/// Memory of the in-kernel test VMs (`ktest`, `ktest-smp`), in MiB: the
/// packages cell's files (numpy) are embedded, decoded and mapped (design
/// D4 of `add-pip-packages`).
pub(crate) const KTEST_MEMORY_MIB: u32 = 512;

/// Virtual CPUs of the `ktest-smp` step (spec `multi-core`).
pub(crate) const SMP_CPUS: u32 = 4;
/// Timeout for the self-check that expects a hanging kernel.
const HANG_CHECK_TIMEOUT_SECS: u64 = 5;

const HELP: &str = "\
Usage: cargo xtask <command> [options]

Commands:
  check                      Fast loop (seconds, no VM): fmt, clippy, host
                             tests, crate dependency rules
  build [--release]          Build the kernel image
  run [options]              Build and boot the kernel, console on stdout,
                             then report the time to ready
      --release              Optimized build
      --vmm <name>           qemu-microvm (default), qemu-q35, firecracker,
                             cloud-hypervisor
      --machine <microvm|q35>  Shorthand for the QEMU VMMs
      --accel <auto|kvm|tcg|icount>
                             QEMU acceleration (default auto: KVM if usable;
                             icount: emulated, clock counts instructions)
      --gdb                  QEMU only: wait for a debugger on localhost:1234
  test [--vmm <name>] [--accel <a>] [--only <step>]
                             Host tests, boot check, in-kernel tests and
                             harness self-checks; --only runs one step:
                             host, boot, ktest, ktest-smp (4 CPUs),
                             control (`rhizo` against a control kernel) or
                             a self-check name (e.g. ktest-page-fault)
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
      --no-icount            Skip the instruction counts under QEMU -icount
      --workload <laptop|server>
                             Only this reference workload (both run by
                             default, after the cell figures)
      --soak                 The long versions of the workloads (weekly)
      --cells <n>            Cells of the server workload (default 10000)
      --llm-delay-ms <n>     Delay of the laptop's fake model (default 0)
  profile [options]          Run the workloads (cell loop, fork_touch, CPython
                             cold) sampled and accounted; write folded stacks
                             per workload (cells' functions, kernel parts) and
                             per system call
      --vmm <name>           VMM (default: QEMU's default machine)
      --accel <a>            QEMU acceleration
      --out <dir>            Where to write them (default target/profile)
      --per-cell             One frame per cell under the image
  ci                         Everything CI runs: fmt, clippy, test
  demo agent --llm <url> --model <name> [--vmm <name>] [--accel <a>]
                             The CPython agent in a cell against a real
                             OpenAI-compatible model (Ollama, llama-server),
                             shown live: `rhizo up`, `rhizo run
                             python-agent`, `rhizo down`; needs
                             RHIZO_CPYTHON_SRC (docs/demo.md)
  mutants                    Mutation testing of rhizo-mm and rhizo-core
                             (needs cargo-mutants; slow, runs weekly in CI)
  help                       Show this message

Global options:
  --arch <aarch64|x86_64>    Kernel architecture (default aarch64)
  --json                     One JSON object per line on stdout (step,
                             failure, data, summary); other output on stderr

Environment:
  RHIZO_TEST_TIMEOUT         Seconds before a test VM is killed (default 30)
  RHIZO_QEMU                 QEMU binary (default qemu-system-x86_64)
  RHIZO_FIRECRACKER, RHIZO_CLOUD_HYPERVISOR, RHIZO_REFERENCE_KERNEL
                             Use local files instead of downloading
";

/// `cargo xtask`: runs the command its arguments name.
pub fn main() -> ExitCode {
    let mut args: Vec<String> = env::args().skip(1).collect();
    report::set_json(args.iter().any(|a| a == "--json"));
    args.retain(|a| a != "--json");
    if let Some(i) = args.iter().position(|a| a == "--arch") {
        match args.get(i + 1).map(|name| arch::Arch::parse(name)) {
            Some(Ok(selected)) => arch::select(selected),
            Some(Err(e)) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
            None => {
                eprintln!("error: --arch needs a value");
                return ExitCode::FAILURE;
            }
        }
        args.drain(i..i + 2);
    }
    let rest = args.get(1..).unwrap_or_default();
    let result = match args.first().map(String::as_str) {
        Some("check") => cmd_check(),
        Some("build") => {
            build_kernel(Profile::from_args(rest), &[]).map(|path| println!("{}", path.display()))
        }
        Some("run") => RunArgs::parse(rest).and_then(|a| cmd_run(&a)),
        Some("test") => RunArgs::parse(rest).and_then(|a| cmd_test(&a)),
        Some("bench") => bench::Options::parse(rest).and_then(|o| bench::cmd(&o)),
        Some("profile") => profile::ProfileArgs::parse(rest).and_then(|a| profile::cmd(&a)),
        Some("ci") => cmd_ci(),
        Some("demo") => demo::cmd(rest),
        Some("mutants") => cmd_mutants(),
        Some("help") | Some("--help") | Some("-h") | None => {
            print!("{HELP}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{HELP}")),
    };
    match result {
        Ok(()) => {
            report::summary(true, "all checks passed");
            ExitCode::SUCCESS
        }
        Err(message) => {
            if report::json() {
                report::summary(false, &message);
            } else {
                eprintln!("\n{message}");
            }
            ExitCode::FAILURE
        }
    }
}

pub type Result<T = ()> = std::result::Result<T, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
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
    /// Run only this step of `test`.
    only: Option<String>,
}

impl RunArgs {
    fn parse(args: &[String]) -> Result<Self> {
        let mut parsed = Self {
            profile: Profile::from_args(args),
            vmm: Vmm::default_for(arch::current()),
            accel: Accel::Auto,
            gdb: false,
            only: None,
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
                        "virt" => Vmm::QemuVirt,
                        "microvm" => Vmm::QemuMicrovm,
                        "q35" => Vmm::QemuQ35,
                        other => {
                            return Err(format!(
                                "unknown machine `{other}` (virt, microvm or q35)"
                            ));
                        }
                    }
                }
                "--accel" => parsed.accel = Accel::parse(&value("--accel")?)?,
                "--only" => {
                    let step = value("--only")?;
                    if !test_steps().contains(&step.as_str()) {
                        return Err(format!(
                            "unknown step `{step}`; expected one of: {}",
                            test_steps().join(", ")
                        ));
                    }
                    parsed.only = Some(step);
                }
                other => return Err(format!("unknown option `{other}`")),
            }
        }
        if parsed.gdb && parsed.vmm.requires_kvm() {
            return Err("--gdb is only supported on the QEMU VMMs".into());
        }
        Ok(parsed)
    }

    fn wants(&self, step: &str) -> bool {
        self.only.as_deref().is_none_or(|only| only == step)
    }

    /// Command that reruns `step` with the same VMM.
    fn reproduce(&self, step: &str) -> String {
        let mut cmd = format!(
            "cargo xtask --arch {} test --only {step}",
            arch::current().id()
        );
        if self.vmm != Vmm::default_for(arch::current()) {
            cmd += &format!(" --vmm {}", self.vmm.id());
        }
        cmd
    }

    fn run_options(&self, timeout: Duration) -> RunOptions {
        RunOptions {
            stop_at_ready: false,
            timeout,
            echo: true,
            accel: self.accel,
            gdb: self.gdb,
            network: false,
            pci_card: false,
            cpus: 1,
            memory_mib: vmm::DEFAULT_MEMORY_MIB,
            bench: None,
            shard: None,
            control: false,
        }
    }
}

fn cmd_run(args: &RunArgs) -> Result {
    let kernel = build_kernel(args.profile, &[])?;
    if args.gdb {
        report::text("QEMU is waiting for gdb on localhost:1234");
    }
    let timeout = if args.gdb {
        Duration::from_secs(24 * 3600)
    } else {
        test_timeout()?
    };
    let run = vmm::run(args.vmm, &Guest::Rhizo(kernel), args.run_options(timeout))?;
    report_timing(args.vmm, &run);
    let mut cmd = format!("cargo xtask run --vmm {}", args.vmm.id());
    if args.profile == Profile::Release {
        cmd += " --release";
    }
    check_vm(
        run.result == VmResult::Passed,
        VmCheck {
            kind: "boot",
            what: format!("kernel run on {} must succeed", args.vmm.id()),
            expected: "Passed".into(),
            reproduce: cmd,
        },
        &run,
    )
}

/// Prints the host-measured time to ready and the kernel's own cycle count.
fn report_timing(vmm: Vmm, run: &VmRun) {
    match run.time_to_ready {
        Some(t) => {
            report::text(&format!(
                "[xtask] {}: time to ready {:.2} ms (host), {} kernel cycles",
                vmm.id(),
                t.as_secs_f64() * 1000.0,
                run.ready_cycles
                    .map_or_else(|| "unknown".to_string(), |c| c.to_string())
            ));
            report::data(
                "timing",
                &serde_json::json!({
                    "vmm": vmm.id(),
                    "time_to_ready_ms": t.as_secs_f64() * 1000.0,
                    "kernel_cycles": run.ready_cycles,
                }),
            );
        }
        None => report::text(&format!(
            "[xtask] {}: the ready marker never arrived",
            vmm.id()
        )),
    }
}

/// Names accepted by `test --only`.
fn test_steps() -> Vec<&'static str> {
    let mut steps = vec![
        "host",
        "differential",
        "boot",
        "ktest",
        "ktest-smp",
        "control",
    ];
    steps.extend(SELF_CHECKS.iter().map(|c| c.feature));
    steps
}

fn cmd_test(args: &RunArgs) -> Result {
    let host_args = [
        "test",
        "--workspace",
        "--exclude",
        KERNEL_PACKAGE,
        "--exclude",
        CELLS_PACKAGE,
    ];
    let parallel = parallelism(args);
    if args.wants("host") && parallel == 1 {
        step("host tests");
        cargo(&host_args)?;
    }

    if args.wants("differential") {
        step("cells on the host (differential check against Linux)");
        match differential::unavailable_reason() {
            Some(reason) => report::text(&format!("skipped: {reason}")),
            None => differential::check(&build_cells()?)?,
        }
    }

    // Build everything first (cargo builds one thing at a time anyway),
    // then run the VMs, several at once where the VMM allows it (spec
    // `kernel-testing`, fast feedback).
    let timeout = test_timeout()?;
    // VMs that share the host's CPUs run slower; the hang limit (time
    // without console output) grows with them.
    let opts = args.run_options(if parallel > 1 { timeout * 2 } else { timeout });
    let mut jobs = Vec::new();
    if args.wants("host") && parallel > 1 {
        step("host tests (build)");
        let mut no_run = host_args.to_vec();
        no_run.push("--no-run");
        cargo(&no_run)?;
        jobs.push(Job::host(&host_args));
    }
    if args.wants("boot") {
        let kernel = keep_kernel("boot", &build_kernel(Profile::Debug, &[])?)?;
        jobs.push(Job::vm(
            format!("boot check on {}", args.vmm.id()),
            kernel,
            opts,
            Check::Boot,
            args.reproduce("boot"),
        ));
    }
    let ktest =
        || -> Result<PathBuf> { keep_kernel("ktest", &build_kernel(Profile::Debug, &["ktest"])?) };
    let ktest_kernel = if args.wants("ktest") || args.wants("ktest-smp") {
        Some(ktest()?)
    } else {
        None
    };
    if let (true, Some(kernel)) = (args.wants("ktest"), &ktest_kernel) {
        let shards = shards(parallel);
        for i in 0..shards {
            jobs.push(Job::vm(
                shard_name("in-kernel tests", i, shards),
                kernel.clone(),
                RunOptions {
                    network: true,
                    memory_mib: KTEST_MEMORY_MIB,
                    shard: shard(i, shards),
                    ..opts
                },
                Check::Ktest { cpus: 1 },
                args.reproduce("ktest"),
            ));
        }
    }
    if let (true, Some(kernel)) = (args.wants("ktest-smp"), &ktest_kernel) {
        // Each of these VMs has several CPUs: fewer of them at once.
        let shards = shards(parallel / 2);
        for i in 0..shards {
            jobs.push(Job::vm(
                shard_name(&format!("in-kernel tests on {SMP_CPUS} CPUs"), i, shards),
                kernel.clone(),
                RunOptions {
                    network: true,
                    // QEMU `virt` takes its card over virtio-pci here and over
                    // virtio-mmio in `ktest`, so both transports run on AArch64.
                    pci_card: true,
                    cpus: SMP_CPUS,
                    memory_mib: KTEST_MEMORY_MIB,
                    shard: shard(i, shards),
                    ..opts
                },
                Check::Ktest { cpus: SMP_CPUS },
                args.reproduce("ktest-smp"),
            ));
        }
    }
    if args.wants("control") {
        let kernel = keep_kernel("control", &build_kernel(Profile::Debug, &["control"])?)?;
        jobs.push(Job {
            name: format!("control plane through `rhizo` on {}", args.vmm.id()),
            work: JobWork::Control {
                kernel,
                rhizo: control::build_rhizo()?,
                accel: args.accel,
                reproduce: args.reproduce("control"),
            },
        });
    }
    for check in SELF_CHECKS.iter().filter(|c| args.wants(c.feature)) {
        let kernel = keep_kernel(
            check.feature,
            &build_kernel(Profile::Debug, &[check.feature])?,
        )?;
        let limit = check
            .timeout_secs
            .map(Duration::from_secs)
            .unwrap_or(timeout);
        jobs.push(Job::vm(
            format!("self-check: {}", check.description),
            kernel,
            RunOptions {
                timeout: limit,
                ..opts
            },
            Check::Fails(check),
            args.reproduce(check.feature),
        ));
    }
    run_jobs(args.vmm, &jobs, parallel)
}

/// Most in-kernel test VMs one step is split into.
const MAX_SHARDS: usize = 4;

/// How many VMs share the in-kernel tests of one step: `RHIZO_SHARDS`
/// (at most [`MAX_SHARDS`] and `parallel`), 1 by default. Splitting runs
/// tests without the ones before them, which still exposes tests that
/// depend on their order (`openspec/changes/speed-up-feedback-loop`), so it
/// is opt-in until they are fixed.
fn shards(parallel: usize) -> usize {
    env::var("RHIZO_SHARDS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, MAX_SHARDS.min(parallel.max(1)))
}

/// How many test VMs run at once: one where VMs share a host device (the
/// tap of Firecracker and Cloud Hypervisor) or a debugger waits, otherwise
/// `RHIZO_JOBS` or the host's CPUs.
fn parallelism(args: &RunArgs) -> usize {
    if args.gdb || matches!(args.vmm, Vmm::Firecracker | Vmm::CloudHypervisor) {
        return 1;
    }
    env::var("RHIZO_JOBS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
}

fn shard(i: usize, n: usize) -> Option<(u16, u16)> {
    (n > 1).then_some((i as u16, n as u16))
}

fn shard_name(name: &str, i: usize, n: usize) -> String {
    if n > 1 {
        format!("{name} (part {} of {n})", i + 1)
    } else {
        name.to_string()
    }
}

/// Copies a freshly built kernel image aside, so building the next kernel
/// does not replace it before its VM runs.
fn keep_kernel(name: &str, image: &Path) -> Result<PathBuf> {
    let dir = target_dir().join("xtask-test").join(arch::current().id());
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let out = dir.join(match image.extension() {
        Some(ext) => format!("{name}.{}", ext.to_string_lossy()),
        None => name.to_string(),
    });
    std::fs::copy(image, &out)
        .map_err(|e| format!("cannot copy {} to {}: {e}", image.display(), out.display()))?;
    Ok(out)
}

/// What a test VM must show.
#[derive(Clone, Copy)]
enum Check {
    Boot,
    Ktest { cpus: u32 },
    Fails(&'static SelfCheck),
}

/// One step of `test` that can run alongside others.
struct Job {
    name: String,
    work: JobWork,
}

enum JobWork {
    Host(Vec<String>),
    /// The control kernel driven through `rhizo` (`control::check`).
    Control {
        kernel: PathBuf,
        rhizo: PathBuf,
        accel: Accel,
        reproduce: String,
    },
    Vm {
        kernel: PathBuf,
        opts: RunOptions,
        check: Check,
        reproduce: String,
    },
}

impl Job {
    fn host(args: &[&str]) -> Self {
        Job {
            name: "host tests".into(),
            work: JobWork::Host(args.iter().map(|a| a.to_string()).collect()),
        }
    }

    /// Host CPUs the job keeps busy: one per virtual CPU of a VM (emulation
    /// runs one host thread per virtual CPU); a few for `cargo test`.
    fn weight(&self) -> usize {
        match &self.work {
            JobWork::Host(_) | JobWork::Control { .. } => 2,
            // A VM with several CPUs takes half the host at most, so the
            // single-CPU tests run beside it (its CPUs are mostly idle).
            JobWork::Vm { opts, .. } => opts.cpus as usize,
        }
    }

    fn vm(
        name: String,
        kernel: PathBuf,
        opts: RunOptions,
        check: Check,
        reproduce: String,
    ) -> Self {
        Job {
            name,
            work: JobWork::Vm {
                kernel,
                opts,
                check,
                reproduce,
            },
        }
    }

    /// Runs the job; with `quiet`, output is kept and shown only on failure.
    fn run(&self, vmm: Vmm, quiet: bool) -> Result {
        match &self.work {
            JobWork::Host(args) => {
                let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                if !quiet {
                    return cargo(&refs);
                }
                let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
                let out = Command::new(&cargo)
                    .args(&refs)
                    .current_dir(workspace_root())
                    .output()
                    .map_err(|e| format!("cannot run {cargo}: {e}"))?;
                if out.status.success() {
                    return Ok(());
                }
                report::text(&String::from_utf8_lossy(&out.stdout));
                report::text(&String::from_utf8_lossy(&out.stderr));
                Err(Failure::new("host-tests", "host tests must pass")
                    .expected("cargo test exits successfully")
                    .actual(out.status.to_string())
                    .hint("the failing test and its message are printed above")
                    .reproduce(format!("cargo {}", refs.join(" ")))
                    .into_error())
            }
            JobWork::Control {
                kernel,
                rhizo,
                accel,
                reproduce,
            } => control::check(vmm, *accel, kernel, rhizo, reproduce),
            JobWork::Vm {
                kernel,
                opts,
                check,
                reproduce,
            } => {
                let opts = RunOptions {
                    echo: !quiet,
                    ..*opts
                };
                let run = vmm::run(vmm, &Guest::Rhizo(kernel.clone()), opts)?;
                let verdict = verify(vmm, *check, reproduce, &run);
                if verdict.is_err() && quiet {
                    report::text(&format!("[xtask] console output of `{}`:", self.name));
                    report::text(&run.output);
                }
                verdict
            }
        }
    }
}

/// Runs `jobs` on `parallel` host CPUs (each job takes [`Job::weight`] of
/// them, so the host is never oversubscribed and timing-sensitive tests see
/// the speed they would alone), reporting each as it ends; fails with the
/// first failure (in job order) after all have run.
fn run_jobs(vmm: Vmm, jobs: &[Job], parallel: usize) -> Result {
    if parallel <= 1 {
        for job in jobs {
            step(&job.name);
            job.run(vmm, false)?;
        }
        return Ok(());
    }
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Condvar, Mutex, mpsc};
    let next = AtomicUsize::new(0);
    let free = (Mutex::new(parallel), Condvar::new());
    let (tx, rx) = mpsc::channel();
    let mut results: Vec<Option<Result>> = jobs.iter().map(|_| None).collect();
    std::thread::scope(|scope| {
        for _ in 0..parallel.min(jobs.len()) {
            let tx = tx.clone();
            let (next, free) = (&next, &free);
            scope.spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = jobs.get(i) else { break };
                    let need = job.weight().clamp(1, (parallel / 2).max(1));
                    let mut cpus = free.0.lock().unwrap_or_else(|e| e.into_inner());
                    while *cpus < need {
                        cpus = free.1.wait(cpus).unwrap_or_else(|e| e.into_inner());
                    }
                    *cpus -= need;
                    drop(cpus);
                    let started = Instant::now();
                    let result = job.run(vmm, true);
                    *free.0.lock().unwrap_or_else(|e| e.into_inner()) += need;
                    free.1.notify_all();
                    let _ = tx.send((i, started.elapsed(), result));
                }
            });
        }
        drop(tx);
        for (i, took, result) in rx {
            let verdict = if result.is_ok() { "ok" } else { "FAILED" };
            step(&format!(
                "{} ... {verdict} ({:.1} s)",
                jobs[i].name,
                took.as_secs_f64()
            ));
            results[i] = Some(result);
        }
    });
    results
        .into_iter()
        .flatten()
        .find(Result::is_err)
        .unwrap_or(Ok(()))
}

/// Checks a test VM's outcome.
fn verify(vmm: Vmm, check: Check, reproduce: &str, run: &VmRun) -> Result {
    match check {
        Check::Boot => {
            report_timing(vmm, run);
            let check = |what: &str, expected: &str| VmCheck {
                kind: "boot",
                what: what.into(),
                expected: expected.into(),
                reproduce: reproduce.into(),
            };
            check_vm(
                run.result == VmResult::Passed,
                check("boot must exit successfully", "Passed"),
                run,
            )?;
            for needle in [
                "Rhizo ",
                "\nmemory: ",
                "\nrhizo-ready cycles=",
                "boot complete",
            ] {
                check_vm(
                    run.output.contains(needle),
                    check(
                        &format!("boot output must contain `{}`", needle.trim()),
                        &format!("a line containing `{}`", needle.trim()),
                    ),
                    run,
                )?;
            }
            Ok(())
        }
        Check::Ktest { cpus: 1 } => check_vm(
            run.result == VmResult::Passed && run.output.contains("test result: ok."),
            VmCheck {
                kind: "ktest",
                what: "in-kernel tests must pass".into(),
                expected: "Passed with `test result: ok.`".into(),
                reproduce: reproduce.into(),
            },
            run,
        ),
        Check::Ktest { cpus } => {
            let online = format!("cpus: {cpus} online");
            check_vm(
                run.result == VmResult::Passed
                    && run.output.contains("test result: ok.")
                    && run.output.contains(&online),
                VmCheck {
                    kind: "ktest",
                    what: format!("in-kernel tests must pass with {cpus} CPUs online"),
                    expected: format!("`{online}` and Passed with `test result: ok.`"),
                    reproduce: reproduce.into(),
                },
                run,
            )
        }
        Check::Fails(check) => {
            let vm_check = |what: String, expected: String| VmCheck {
                kind: "self-check",
                what,
                expected,
                reproduce: reproduce.into(),
            };
            check_vm(
                run.result == check.expected,
                vm_check(
                    format!("{} must end with {:?}", check.feature, check.expected),
                    format!("{:?}", check.expected),
                ),
                run,
            )?;
            for needle in check.expected_output() {
                check_vm(
                    run.output.contains(needle),
                    vm_check(
                        format!("{} output must contain `{needle}`", check.feature),
                        format!("output containing `{needle}`"),
                    ),
                    run,
                )?;
            }
            Ok(())
        }
    }
}

/// Validates `.github/workflows` with actionlint when it is installed.
fn check_workflows() -> Result {
    let status = Command::new("actionlint")
        .current_dir(workspace_root())
        .stdout(report::child_stdout())
        .status();
    match status {
        Err(_) => {
            report::text("[xtask] actionlint is not installed; skipped (CI runs it)");
            Ok(())
        }
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(
            Failure::new("workflows", "GitHub Actions workflow files are invalid")
                .location(".github/workflows/")
                .expected("no actionlint findings")
                .actual(s.to_string())
                .hint("fix each finding at the file:line actionlint printed above")
                .reproduce("actionlint")
                .into_error(),
        ),
    }
}

/// Mutation testing: every surviving mutant is a change no test notices.
fn cmd_mutants() -> Result {
    step("mutation testing (rhizo-mm, rhizo-core)");
    let status = Command::new("cargo")
        .args([
            "mutants",
            "--package",
            "rhizo-mm",
            "--package",
            "rhizo-core",
            "--timeout",
            "120",
        ])
        .current_dir(workspace_root())
        .stdout(report::child_stdout())
        .status()
        .map_err(|e| {
            Failure::new("tool", format!("cannot run cargo-mutants: {e}"))
                .hint("install it with `cargo install --locked cargo-mutants`")
                .reproduce("cargo xtask mutants")
                .into_error()
        })?;
    if status.success() {
        return Ok(());
    }
    Err(Failure::new(
        "mutants",
        "some mutants survived: tests did not detect these changes",
    )
    .location("mutants.out/missed.txt")
    .expected("every mutant caught by a test")
    .actual(status.to_string())
    .hint("for each missed mutant, add a test that fails on that change (do not weaken the code)")
    .reproduce("cargo xtask mutants")
    .into_error())
}

/// The fast loop: no VM, no release build.
fn cmd_check() -> Result {
    let started = Instant::now();
    step("format");
    cargo(&["fmt", "--all", "--check"])?;
    step("clippy (host crates)");
    cargo(&[
        "clippy",
        "--workspace",
        "--exclude",
        KERNEL_PACKAGE,
        "--exclude",
        CELLS_PACKAGE,
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ])?;
    for arch in [arch::Arch::Aarch64, arch::Arch::X86_64] {
        step(&format!("clippy (cells, {})", arch.id()));
        cargo(&[
            "clippy",
            "-p",
            CELLS_PACKAGE,
            "--target",
            arch.target(),
            "--",
            "-D",
            "warnings",
        ])?;
        step(&format!("clippy (kernel, {})", arch.id()));
        cargo(&[
            "clippy",
            "-p",
            KERNEL_PACKAGE,
            "--target",
            arch.target(),
            "--features",
            "ktest",
            "--",
            "-D",
            "warnings",
        ])?;
    }
    step("host tests");
    cargo(&[
        "test",
        "--workspace",
        "--exclude",
        KERNEL_PACKAGE,
        "--exclude",
        CELLS_PACKAGE,
    ])?;
    step("crate dependency rules");
    deps::check()?;
    step("workflow files (actionlint)");
    check_workflows()?;
    let secs = started.elapsed().as_secs_f64();
    report::text(&format!("[xtask] check took {secs:.1} s"));
    report::data(
        "duration",
        &serde_json::json!({"command": "check", "seconds": secs}),
    );
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
        "--exclude",
        CELLS_PACKAGE,
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ])?;

    let features = [""]
        .into_iter()
        .chain(["ktest", "bench-cell", "bench-cell,profile", "control"])
        .chain(SELF_CHECKS.iter().map(|c| c.feature));
    step("clippy (cells)");
    cargo(&[
        "clippy",
        "-p",
        CELLS_PACKAGE,
        "--target",
        arch::current().target(),
        "--",
        "-D",
        "warnings",
    ])?;
    for features in features {
        step(&format!("clippy (kernel, features: `{features}`)"));
        let target = arch::current().target();
        let mut args = vec!["clippy", "-p", KERNEL_PACKAGE, "--target", target];
        if !features.is_empty() {
            args.extend(["--features", features]);
        }
        args.extend(["--", "-D", "warnings"]);
        cargo(&args)?;
    }

    step("crate dependency rules");
    deps::check()?;

    cmd_test(&RunArgs::parse(&[])?)?;
    if arch::current() != arch::Arch::X86_64 {
        return Ok(());
    }

    step("boot check on qemu-q35");
    let kernel = build_kernel(Profile::Debug, &[])?;
    let q35 = RunArgs::parse(&["--machine".into(), "q35".into()])?;
    let run = vmm::run(
        Vmm::QemuQ35,
        &Guest::Rhizo(kernel),
        q35.run_options(test_timeout()?),
    )?;
    report_timing(Vmm::QemuQ35, &run);
    check_vm(
        run.result == VmResult::Passed,
        VmCheck {
            kind: "boot",
            what: "q35 boot must exit successfully".into(),
            expected: "Passed".into(),
            reproduce: "cargo xtask --arch x86_64 test --only boot --vmm qemu-q35".into(),
        },
        &run,
    )?;

    // q35 is the x86_64 machine whose network card is on PCI.
    step("in-kernel tests on qemu-q35 (virtio-pci card)");
    let kernel = build_kernel(Profile::Debug, &["ktest"])?;
    let run = vmm::run(
        Vmm::QemuQ35,
        &Guest::Rhizo(kernel),
        RunOptions {
            network: true,
            memory_mib: KTEST_MEMORY_MIB,
            ..q35.run_options(test_timeout()?)
        },
    )?;
    check_vm(
        run.result == VmResult::Passed
            && run.output.contains("test result: ok.")
            && run.output.contains("at virtio-pci "),
        VmCheck {
            kind: "ktest",
            what: "in-kernel tests must pass on q35 with a virtio-pci card".into(),
            expected: "`net: card ... at virtio-pci` and Passed with `test result: ok.`".into(),
            reproduce: "cargo xtask --arch x86_64 test --only ktest --vmm qemu-q35".into(),
        },
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
    /// Expected output on x86_64.
    output: &'static [&'static str],
    /// Expected output on AArch64, when it differs (exception reports).
    output_aarch64: Option<&'static [&'static str]>,
}

impl SelfCheck {
    fn expected_output(&self) -> &'static [&'static str] {
        match (arch::current(), self.output_aarch64) {
            (arch::Arch::Aarch64, Some(output)) => output,
            _ => self.output,
        }
    }
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
        output_aarch64: None,
    },
    SelfCheck {
        feature: "ktest-hang",
        description: "a hanging test times out",
        expected: VmResult::TimedOut,
        timeout_secs: Some(HANG_CHECK_TIMEOUT_SECS),
        output: &["test hang_forever ... "],
        output_aarch64: None,
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
        output_aarch64: Some(&[
            "EXCEPTION: data abort (class 0x25",
            "address 0x0: write, translation fault",
        ]),
    },
    SelfCheck {
        feature: "ktest-stack-overflow",
        description: "stack exhaustion is reported, not a silent reset",
        expected: VmResult::Failed,
        timeout_secs: None,
        output: &["EXCEPTION: double fault (vector 8"],
        output_aarch64: Some(&[
            "EXCEPTION: data abort (class 0x25",
            "kernel stack overflow (guard page hit)",
        ]),
    },
    SelfCheck {
        feature: "ktest-invalid-opcode",
        description: "an invalid instruction is reported",
        expected: VmResult::Failed,
        timeout_secs: None,
        output: &["EXCEPTION: invalid opcode (vector 6"],
        output_aarch64: Some(&["EXCEPTION: undefined instruction (class 0x0"]),
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
        output_aarch64: None,
    },
    SelfCheck {
        feature: "ktest-user-read",
        description: "the kernel reading a user page faults (SMAP, PAN)",
        expected: VmResult::Failed,
        timeout_secs: None,
        output: &[
            "EXCEPTION: page fault (vector 14",
            "address 0x400000: read, protection violation, kernel mode",
        ],
        output_aarch64: Some(&[
            "EXCEPTION: data abort (class 0x25",
            "address 0x400000: read, permission fault",
        ]),
    },
    SelfCheck {
        feature: "ktest-user-exec",
        description: "the kernel executing a user page faults (SMEP, PXN)",
        expected: VmResult::Failed,
        timeout_secs: None,
        output: &[
            "EXCEPTION: page fault (vector 14",
            "address 0x400000: instruction fetch, protection violation, kernel mode",
        ],
        output_aarch64: Some(&[
            "EXCEPTION: instruction abort (class 0x21",
            "address 0x400000: instruction fetch, permission fault",
        ]),
    },
];

// ---------------------------------------------------------------------------
// Building
// ---------------------------------------------------------------------------

/// Builds the kernel for the selected architecture and returns the image the
/// VMMs boot (the ELF on x86_64, a flat arm64 Image on AArch64).
pub fn build_kernel(profile: Profile, features: &[&str]) -> Result<PathBuf> {
    let target = arch::current().target();
    let cells = build_cells()?;
    let mut args = vec!["build", "-p", KERNEL_PACKAGE, "--target", target];
    if profile == Profile::Release {
        args.push("--release");
    }
    let joined = features.join(",");
    if !features.is_empty() {
        args.extend(["--features", &joined]);
    }
    cargo_env(&args, &[("RHIZO_CELLS_DIR", cells.as_os_str())])?;
    arch::current().bootable_image(&kernel_elf(profile))
}

/// Builds the test cells (`cells/`, static Linux executables for the selected
/// architecture) and returns the directory holding them. They have their own
/// target directory so their link flags never invalidate the kernel build.
pub(crate) fn build_cells() -> Result<PathBuf> {
    let target = arch::current().target();
    let dir = target_dir().join("cells");
    let dir_str = dir.to_string_lossy().into_owned();
    cargo(&[
        "build",
        "-p",
        CELLS_PACKAGE,
        "--target",
        target,
        "--release",
        "--target-dir",
        &dir_str,
    ])?;
    let out = dir.join(target).join("release");
    build_c_cells(&out)?;
    build_micropython(&out)?;
    cpython::build(&out)?;
    dynamic::build(&out)?;
    packages::build(&out)?;
    Ok(out)
}

/// MicroPython's commit, pinned (v1.24.1, MIT license).
pub(crate) const MICROPYTHON_COMMIT: &str = "ecfdd5d6f9be971852003c2049600dc7b3e2a838";

/// Builds MicroPython's Unix port as a static executable into `out` when its
/// source is given (`RHIZO_MICROPYTHON_SRC`, a checkout of
/// [`MICROPYTHON_COMMIT`]); without it, the MicroPython checks report that
/// they did not run. Options left out need what cells do not have yet
/// (threads, sockets, FFI) or are not needed (TLS, readline, frozen code).
fn build_micropython(out: &Path) -> Result {
    let target = out.join("micropython");
    let Some(src) = env::var_os("RHIZO_MICROPYTHON_SRC") else {
        let _ = std::fs::remove_file(&target);
        return Ok(());
    };
    let src = PathBuf::from(src);
    let head = Command::new("git")
        .arg("-C")
        .arg(&src)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
    if head != MICROPYTHON_COMMIT {
        return Err(
            Failure::new("build", "MicroPython is not the pinned commit")
                .location(src.display().to_string())
                .expected(MICROPYTHON_COMMIT)
                .actual(head)
                .hint("check out the pinned commit, or update MICROPYTHON_COMMIT on purpose")
                .reproduce(format!(
                    "git -C {} checkout {MICROPYTHON_COMMIT}",
                    src.display()
                ))
                .into_error(),
        );
    }
    if target.exists() {
        return Ok(());
    }
    let port = src.join("ports").join("unix");
    let cc = format!("CC={}", c_compiler());
    let args = [
        "VARIANT=standard",
        "MICROPY_PY_FFI=0",
        "MICROPY_PY_THREAD=0",
        "MICROPY_PY_SOCKET=0",
        "MICROPY_PY_SSL=0",
        "MICROPY_PY_BTREE=0",
        "MICROPY_USE_READLINE=0",
        "FROZEN_MANIFEST=",
        "LDFLAGS_EXTRA=-static",
        &cc,
    ];
    let status = Command::new("make")
        .arg("-C")
        .arg(&port)
        .arg(format!(
            "-j{}",
            std::thread::available_parallelism().map_or(2, |n| n.get())
        ))
        .args(args)
        .status()
        .map_err(|e| format!("cannot run make: {e}"))?;
    if !status.success() {
        return Err(Failure::new("build", "cannot build MicroPython")
            .location(port.display().to_string())
            .expected("a static micropython executable")
            .actual(status.to_string())
            .reproduce(format!("make -C {} {}", port.display(), args.join(" ")))
            .into_error());
    }
    std::fs::copy(port.join("build-standard").join("micropython"), &target)
        .map_err(|e| format!("cannot copy micropython: {e}"))?;
    Ok(())
}

/// C compiler producing static Linux executables for the selected
/// architecture: the native one when the host matches, else the cross gcc.
pub(crate) fn c_compiler() -> &'static str {
    match (arch::current(), env::consts::ARCH) {
        (arch::Arch::Aarch64, "aarch64") | (arch::Arch::X86_64, "x86_64") => "cc",
        (arch::Arch::Aarch64, _) => "aarch64-linux-gnu-gcc",
        (arch::Arch::X86_64, _) => "x86_64-linux-gnu-gcc",
    }
}

/// Builds `cells/c/*.c`: real C programs linked statically with the system
/// glibc (the real libc of H6), next to the Rust cells.
fn build_c_cells(out: &Path) -> Result {
    let src = workspace_root().join("cells").join("c");
    let mut sources: Vec<PathBuf> = std::fs::read_dir(&src)
        .map_err(|e| format!("cannot read {}: {e}", src.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "c"))
        .collect();
    sources.sort();
    let cc = c_compiler();
    for source in sources {
        let name = source
            .file_stem()
            .expect("a file name")
            .to_string_lossy()
            .into_owned();
        let target = out.join(&name);
        let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        if modified(&target).is_some() && modified(&target) >= modified(&source) {
            continue;
        }
        let command = format!(
            "{cc} -static -O2 -o {} {}",
            target.display(),
            source.display()
        );
        let status = Command::new(cc)
            .args(["-static", "-O2", "-o"])
            .arg(&target)
            .arg(&source)
            .status();
        if !matches!(&status, Ok(s) if s.success()) {
            return Err(
                Failure::new("build", format!("cannot compile the C cell `{name}`"))
                    .location(source.display().to_string())
                    .expected("a static executable")
                    .actual(match status {
                        Ok(s) => s.to_string(),
                        Err(e) => e.to_string(),
                    })
                    .hint(format!(
                        "C cells need `{cc}` and a static libc (Debian/Ubuntu: `gcc` and \
                         `libc6-dev`, or `gcc-aarch64-linux-gnu` / `gcc-x86-64-linux-gnu` to \
                         cross-compile)"
                    ))
                    .reproduce(command)
                    .into_error(),
            );
        }
    }
    Ok(())
}

/// The linked kernel ELF (with symbols) for the selected architecture.
fn kernel_elf(profile: Profile) -> PathBuf {
    target_dir()
        .join(arch::current().target())
        .join(profile.dir())
        .join(KERNEL_PACKAGE)
}

fn cargo(args: &[&str]) -> Result {
    cargo_env(args, &[])
}

fn cargo_env(args: &[&str], vars: &[(&str, &std::ffi::OsStr)]) -> Result {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(&cargo)
        .args(args)
        .envs(vars.iter().copied())
        .current_dir(workspace_root())
        .stdout(report::child_stdout())
        .status()
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    if status.success() {
        return Ok(());
    }
    let command = format!("cargo {}", args.join(" "));
    let (kind, hint) = match args.first().copied() {
        Some("fmt") => ("fmt", "run `cargo fmt --all` to apply the formatting"),
        Some("clippy") => (
            "clippy",
            "fix each lint at the file:line clippy printed above; do not silence it without a reason",
        ),
        Some("test") => (
            "host-test",
            "the output above names the failing test, its file:line and left/right values; \
             fix the code, not the test, unless the spec changed",
        ),
        _ => (
            "build",
            "the compiler output above shows the error and its file:line",
        ),
    };
    Err(Failure::new(kind, format!("`{command}` failed"))
        .location(&command)
        .expected("exit status 0")
        .actual(status.to_string())
        .hint(hint)
        .reproduce(&command)
        .into_error())
}

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the workspace")
        .to_path_buf()
}

pub fn target_dir() -> PathBuf {
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
    report::step(name);
}

/// A check on the outcome of a VM run.
struct VmCheck {
    kind: &'static str,
    what: String,
    expected: String,
    reproduce: String,
}

/// Turns a failed VM check into an actionable failure report.
fn check_vm(ok: bool, check: VmCheck, run: &VmRun) -> Result {
    if ok {
        return Ok(());
    }
    let mut failure = Failure::new(check.kind, check.what)
        .expected(check.expected)
        .reproduce(check.reproduce);
    let mut actual = format!("{:?}", run.result);
    if let Some((location, message)) = report::panic_location(&run.output) {
        failure = failure.location(location).hint(
            "the kernel panicked at that location; read the assertion message and fix the \
             code it checks",
        );
        actual += &format!(", panic: {message}");
    } else if let Some((summary, rip)) = report::exception_location(&run.output) {
        failure = failure.location(format!("rip {rip}")).hint(format!(
            "map the address to source with: addr2line -f -e {} {rip}",
            kernel_elf(Profile::Debug).display()
        ));
        actual += &format!(", exception: {summary}");
    } else if run.output.trim().is_empty() {
        failure = failure.hint(
            "the VMM produced no console output: check that it is installed and see the exact \
             command with RHIZO_PRINT_VMM_COMMAND=1",
        );
    } else if run.result == VmResult::TimedOut {
        failure =
            failure.hint("the kernel stopped making progress; the last console line shows where");
    }
    Err(failure.actual(actual).into_error())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_args_parse_vmm_machine_and_accel() {
        let parse =
            |a: &[&str]| RunArgs::parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(
            parse(&["--only", "ktest-page-fault"])
                .unwrap()
                .only
                .as_deref(),
            Some("ktest-page-fault")
        );
        assert!(parse(&["--only", "nope"]).is_err());
        let a = parse(&[]).unwrap();
        assert_eq!(
            (a.vmm, a.accel, a.profile),
            (
                Vmm::default_for(arch::current()),
                Accel::Auto,
                Profile::Debug
            )
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
