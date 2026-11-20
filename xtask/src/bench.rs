//! `cargo xtask bench`: boot benchmarks, baseline gate and README chart
//! (design decisions D7 and D8).

use crate::report::{self, Failure};
use crate::vmm::{self, Accel, Guest, RunOptions, VmResult, Vmm};
use crate::{Profile, Result, build_kernel, refguest, tools, workspace_root};
use serde_json::{Map, Value, json};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const BASELINE: &str = "bench/baseline.json";
const README_START: &str = "<!-- bench:start -->";
const README_END: &str = "<!-- bench:end -->";
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

/// Metrics whose value does not depend on machine noise.
const DETERMINISTIC: [&str; 3] = [
    "image.size_bytes",
    "memory.kernel_pt_frames",
    "cell.pt_frames",
];

/// Options for `cargo xtask bench`.
#[derive(Debug)]
pub struct Options {
    pub runs: usize,
    pub vmms: Vec<Vmm>,
    pub accel: Accel,
    pub env: String,
    pub output: Option<PathBuf>,
    pub from: Option<PathBuf>,
    pub check: bool,
    pub update_baseline: bool,
    pub readme: bool,
    pub reference: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            runs: 20,
            vmms: Vmm::for_arch(crate::arch::current()).to_vec(),
            accel: Accel::Auto,
            env: "local".into(),
            output: None,
            from: None,
            check: false,
            update_baseline: false,
            readme: false,
            reference: true,
        }
    }
}

impl Options {
    /// Parses `bench` arguments.
    pub fn parse(args: &[String]) -> Result<Self> {
        let mut o = Self::default();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let mut value = |name: &str| {
                it.next()
                    .cloned()
                    .ok_or_else(|| format!("{name} needs a value"))
            };
            match arg.as_str() {
                "--runs" => {
                    o.runs = value("--runs")?
                        .parse()
                        .map_err(|_| "--runs needs a positive number".to_string())?;
                    if o.runs == 0 {
                        return Err("--runs needs a positive number".into());
                    }
                }
                "--vmm" => {
                    o.vmms = value("--vmm")?
                        .split(',')
                        .map(Vmm::parse)
                        .collect::<Result<_>>()?;
                }
                "--accel" => o.accel = Accel::parse(&value("--accel")?)?,
                "--env" => o.env = value("--env")?,
                "--output" => o.output = Some(PathBuf::from(value("--output")?)),
                "--from" => o.from = Some(PathBuf::from(value("--from")?)),
                "--check" => o.check = true,
                "--update-baseline" => o.update_baseline = true,
                "--readme" => o.readme = true,
                "--no-reference" => o.reference = false,
                other => return Err(format!("unknown bench option `{other}`")),
            }
        }
        Ok(o)
    }
}

/// One measured metric.
#[derive(Debug, Clone, PartialEq)]
pub struct Metric {
    pub name: String,
    pub guest: String,
    pub vmm: Option<String>,
    pub unit: String,
    pub median: f64,
    pub p90: f64,
    pub runs: usize,
}

impl Metric {
    /// Key used in the baseline: `name`, or `name/guest/vmm`.
    pub fn key(&self) -> String {
        match &self.vmm {
            Some(vmm) => format!("{}/{}/{}", self.name, self.guest, vmm),
            None => self.name.clone(),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "name": self.name, "guest": self.guest, "vmm": self.vmm, "unit": self.unit,
            "median": self.median, "p90": self.p90, "runs": self.runs,
        })
    }

    fn from_json(v: &Value) -> Result<Self> {
        let s = |k: &str| v[k].as_str().map(str::to_string);
        let f = |k: &str| {
            v[k].as_f64()
                .ok_or_else(|| format!("metric field `{k}` missing"))
        };
        Ok(Self {
            name: s("name").ok_or("metric name missing")?,
            guest: s("guest").unwrap_or_else(|| "rhizo".into()),
            vmm: s("vmm"),
            unit: s("unit").unwrap_or_default(),
            median: f("median")?,
            p90: f("p90")?,
            runs: v["runs"].as_u64().unwrap_or(1) as usize,
        })
    }
}

/// Median and 90th percentile of `values` (nearest-rank).
pub fn summarize(values: &mut [f64]) -> (f64, f64) {
    assert!(!values.is_empty(), "no samples");
    values.sort_by(f64::total_cmp);
    let n = values.len();
    let median = if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    };
    let rank = ((0.9 * n as f64).ceil() as usize).clamp(1, n);
    (median, values[rank - 1])
}

/// Results of a benchmark session.
#[derive(Debug)]
pub struct Results {
    pub environment: String,
    pub date: String,
    pub kvm: bool,
    pub metrics: Vec<Metric>,
    pub skipped: Vec<(String, String)>,
}

impl Results {
    fn to_json(&self) -> Value {
        json!({
            "environment": self.environment,
            "date": self.date,
            "kvm": self.kvm,
            "metrics": self.metrics.iter().map(Metric::to_json).collect::<Vec<_>>(),
            "skipped": self.skipped.iter()
                .map(|(vmm, reason)| json!({"vmm": vmm, "reason": reason}))
                .collect::<Vec<_>>(),
        })
    }

    fn from_json(v: &Value) -> Result<Self> {
        Ok(Self {
            environment: v["environment"].as_str().unwrap_or("unknown").into(),
            date: v["date"].as_str().unwrap_or("unknown").into(),
            kvm: v["kvm"].as_bool().unwrap_or(false),
            metrics: v["metrics"]
                .as_array()
                .ok_or("results have no `metrics` array")?
                .iter()
                .map(Metric::from_json)
                .collect::<Result<_>>()?,
            skipped: v["skipped"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|s| {
                            let field = |k: &str| s[k].as_str().unwrap_or_default().to_string();
                            (field("vmm"), field("reason"))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

/// Entry point for `cargo xtask bench`.
pub fn cmd(opts: &Options) -> Result {
    let results = match &opts.from {
        Some(path) => {
            let text = fs::read_to_string(path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let value = serde_json::from_str(&text)
                .map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?;
            Results::from_json(&value)?
        }
        None => measure(opts)?,
    };
    print_table(&results);

    if let Some(path) = &opts.output {
        let text = serde_json::to_string_pretty(&results.to_json()).expect("serializable");
        fs::write(path, text + "\n")
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        report::text(&format!("results written to {}", path.display()));
    }
    if opts.update_baseline {
        update_baseline(&opts.env, &results)?;
    }
    if opts.readme {
        update_readme(&results)?;
    }
    if opts.check {
        check_baseline(&opts.env, &results)?;
    }
    Ok(())
}

fn measure(opts: &Options) -> Result<Results> {
    let kernel = build_kernel(Profile::Release, &[])?;
    let size = fs::metadata(&kernel)
        .map_err(|e| format!("cannot stat {}: {e}", kernel.display()))?
        .len() as f64;

    let mut metrics = vec![deterministic("image.size_bytes", "bytes", size)];
    let mut skipped = Vec::new();
    let mut pt_frames = None;

    let reference = if opts.reference {
        match (tools::reference_kernel(), refguest::write_initramfs()) {
            (Ok(kernel), Ok(initramfs)) => Some(Guest::ReferenceLinux { kernel, initramfs }),
            (Err(e), _) | (_, Err(e)) => {
                skipped.push(("reference-linux".into(), e));
                None
            }
        }
    } else {
        None
    };

    for &vmm in &opts.vmms {
        if let Some(reason) = vmm.unavailable_reason(opts.accel) {
            report::text(&format!("skipping {}: {reason}", vmm.id()));
            skipped.push((vmm.id().into(), reason));
            continue;
        }
        let rhizo = Guest::Rhizo(kernel.clone());
        match sample(vmm, &rhizo, opts.runs, opts.accel) {
            Ok(s) => {
                pt_frames = pt_frames.or(s.pt_frames);
                metrics.extend(s.metrics("rhizo", vmm));
            }
            Err(e) => skipped.push((vmm.id().into(), e)),
        }
        if let Some(reference) = &reference {
            let kvm = vmm::kvm_available() && opts.accel != Accel::Tcg;
            if crate::arch::current() == crate::arch::Arch::Aarch64 && vmm != Vmm::Firecracker {
                // Firecracker's arm64 CI kernel has no PL011 driver, the UART
                // of QEMU virt and Cloud Hypervisor, so it has no console there.
                skipped.push((
                    format!("linux/{}", vmm.id()),
                    "the arm64 reference kernel has no PL011 driver (known issue, see \
                     bench/README.md)"
                        .into(),
                ));
                continue;
            }
            if vmm == Vmm::QemuMicrovm {
                // Observed with and without KVM: the reference kernel often
                // resets right after KASLR setup on QEMU microvm, before /init.
                // Until that is understood, no number is better than noise.
                skipped.push((
                    "linux/qemu-microvm".into(),
                    "the reference Linux kernel resets before /init on QEMU microvm \
                     (known issue, see bench/README.md)"
                        .into(),
                ));
                continue;
            }
            // Linux boots slowly under emulation; keep local runs short.
            let runs = if kvm { opts.runs } else { opts.runs.min(3) };
            match sample(vmm, reference, runs, opts.accel) {
                Ok(s) => metrics.extend(s.metrics("linux", vmm)),
                Err(e) => skipped.push((format!("linux/{}", vmm.id()), e)),
            }
        }
    }
    if let Some(frames) = pt_frames {
        metrics.push(deterministic(
            "memory.kernel_pt_frames",
            "frames",
            frames as f64,
        ));
    }
    measure_cells(opts, &mut metrics, &mut skipped)?;
    measure_comparisons(opts, &mut metrics, &mut skipped)?;
    Ok(Results {
        environment: opts.env.clone(),
        date: today(),
        kvm: vmm::kvm_available(),
        metrics,
        skipped,
    })
}

/// The same operations with existing mechanisms on this machine (spec
/// `comparisons`): Linux processes, and Firecracker snapshot restore. A
/// comparison that cannot run here is skipped with its reason.
fn measure_comparisons(
    opts: &Options,
    metrics: &mut Vec<Metric>,
    skipped: &mut Vec<(String, String)>,
) -> Result {
    let scratch = crate::compare::scratch_dir();
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    match crate::compare::linux_process(&crate::build_cells()?, &scratch) {
        Ok(m) => metrics.extend(m),
        Err(e) => skipped.push(("linux-process".into(), e)),
    }
    if opts.reference {
        match crate::compare::linux_process_in_vm(&crate::build_cells()?, opts.accel, &scratch) {
            Ok(m) => metrics.extend(m),
            Err(e) => skipped.push(("linux-process/firecracker".into(), e)),
        }
        match crate::compare::cpython_in_vm(opts.accel, &scratch) {
            Ok(m) => metrics.extend(m),
            Err(e) => skipped.push(("cpython/linux-process/firecracker".into(), e)),
        }
        match crate::compare::firecracker_restore(opts.runs.clamp(3, 20), opts.accel, &scratch) {
            Ok(m) => metrics.extend(m),
            Err(e) => skipped.push(("firecracker-snapshot".into(), e)),
        }
        match crate::compare::criu(opts.runs.clamp(3, 20), &scratch) {
            Ok(m) => metrics.extend(m),
            Err(e) => skipped.push(("criu".into(), e)),
        }
        match crate::compare::gvisor(&crate::build_cells()?, opts.runs.clamp(3, 5), &scratch) {
            Ok(m) => metrics.extend(m),
            Err(e) => skipped.push(("gvisor".into(), e)),
        }
    }
    Ok(())
}

/// Cell metrics: a `bench-cell` kernel spawns and runs the hello cell many
/// times per boot and prints medians; each VMM boots it a few times.
fn measure_cells(
    opts: &Options,
    metrics: &mut Vec<Metric>,
    skipped: &mut Vec<(String, String)>,
) -> Result {
    let kernel = build_kernel(Profile::Release, &["bench-cell"])?;
    let guest = Guest::Rhizo(kernel);
    let run_opts = RunOptions {
        stop_at_ready: false,
        timeout: RUN_TIMEOUT,
        echo: false,
        accel: opts.accel,
        gdb: false,
        network: true,
        cpus: 1,
    };
    let mut pt_frames = None;
    let mut snapshot_bytes = None;
    let mut fork_pt_frames = None;
    let mut density = None;
    for &vmm in &opts.vmms {
        if vmm.unavailable_reason(opts.accel).is_some() {
            continue; // already reported by the boot benchmark
        }
        report::text(&format!("cells on {}", vmm.id()));
        let (mut spawn, mut run) = (Vec::new(), Vec::new());
        let (mut switch, mut untagged) = (Vec::new(), Vec::new());
        let (mut suspend, mut resume, mut eager, mut touch) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (mut fork, mut fork_touch, mut template) = (Vec::new(), Vec::new(), Vec::new());
        let (mut cold, mut warm) = (Vec::new(), Vec::new());
        let (mut file_write_read, mut fork_files) = (Vec::new(), Vec::new());
        let (mut pipe_rtt, mut loopback_rtt) = (Vec::new(), Vec::new());
        let (mut wire_rtt, mut wire_skipped) = (Vec::new(), None);
        let (mut py_cold, mut py_warm) = (Vec::new(), Vec::new());
        let (mut cpy_cold, mut cpy_warm) = (Vec::new(), Vec::new());
        let (mut thread_create, mut futex_wake) = (Vec::new(), Vec::new());
        for _ in 0..opts.runs.clamp(1, 5) {
            let out = vmm::run(vmm, &guest, run_opts)?;
            match parse_cell_line(&out.output) {
                Some(c) if c.spawn_ns == 0 => {
                    skipped.push((
                        format!("cells/{}", vmm.id()),
                        "the kernel cannot convert cycles to time here (no TSC frequency: \
                         no KVM clock, CPUID leaf or PIT)"
                            .into(),
                    ));
                    break;
                }
                Some(c) if out.result != VmResult::Failed => {
                    spawn.push(c.spawn_ns as f64 / 1000.0);
                    run.push(c.run_ns as f64 / 1000.0);
                    switch.push(c.switch_ns as f64);
                    untagged.push(c.switch_untagged_ns as f64);
                    if let Some((c, w)) = parse_workload_line(&out.output) {
                        cold.push(c as f64 / 1000.0);
                        warm.push(w as f64 / 1000.0);
                    }
                    if let Some(f) = parse_fork_line(&out.output) {
                        fork.push(f.fork_ns as f64 / 1000.0);
                        fork_touch.push(f.fork_touch_ns as f64 / 1000.0);
                        template.push(f.spawn_from_template_ns as f64 / 1000.0);
                        fork_pt_frames = Some(f.fork_pt_frames);
                    }
                    if let Some((c, w)) = parse_python_line(&out.output) {
                        py_cold.push(c as f64 / 1000.0);
                        py_warm.push(w as f64 / 1000.0);
                    }
                    if let Some((c, w)) = parse_cpython_line(&out.output) {
                        cpy_cold.push(c as f64 / 1000.0);
                        cpy_warm.push(w as f64 / 1000.0);
                    }
                    if let Some(d) = parse_density_line(&out.output) {
                        density = Some(d);
                    }
                    if let Some((w, f)) = parse_files_line(&out.output) {
                        file_write_read.push(w as f64 / 1000.0);
                        fork_files.push(f as f64 / 1000.0);
                    }
                    if let Some((p, l)) = parse_net_line(&out.output) {
                        pipe_rtt.push(p as f64);
                        loopback_rtt.push(l as f64 / 1000.0);
                    }
                    match parse_wire_line(&out.output) {
                        Some(Ok(rtt)) => wire_rtt.push(rtt as f64 / 1000.0),
                        Some(Err(why)) => wire_skipped = Some(why),
                        None => {}
                    }
                    if let Some((c, w)) = parse_threads_line(&out.output) {
                        thread_create.push(c as f64 / 1000.0);
                        futex_wake.push(w as f64);
                    }
                    if let Some(snap) = parse_snapshot_line(&out.output) {
                        suspend.push(snap.suspend_ns as f64 / 1000.0);
                        resume.push(snap.resume_ns as f64 / 1000.0);
                        eager.push(snap.resume_eager_ns as f64 / 1000.0);
                        touch.push(snap.resume_touch_ns as f64 / 1000.0);
                        snapshot_bytes = Some(snap.bytes);
                    }
                    pt_frames = Some(c.pt_frames);
                }
                _ => {
                    skipped.push((
                        format!("cells/{}", vmm.id()),
                        format!(
                            "no `rhizo-cell` line ({:?}); output:\n{}",
                            out.result, out.output
                        ),
                    ));
                    break;
                }
            }
        }
        let (mut parallel, mut scaling, mut ipi_wake) = (Vec::new(), Vec::new(), Vec::new());
        let (mut thread_scaling, mut shootdown) = (Vec::new(), Vec::new());
        for _ in 0..opts.runs.clamp(1, 3) {
            let smp = RunOptions {
                cpus: crate::SMP_CPUS,
                ..run_opts
            };
            let out = vmm::run(vmm, &guest, smp)?;
            match parse_smp_line(&out.output) {
                Some(s) if out.result != VmResult::Failed && s.serial_ns > 0 => {
                    let cells = 4.0 * s.cpus as f64;
                    parallel.push(s.parallel_ns as f64 / cells / 1000.0);
                    scaling.push(100.0 * s.parallel_ns as f64 * s.cpus as f64 / s.serial_ns as f64);
                    ipi_wake.push(s.ipi_wake_ns as f64 / 1000.0);
                    if s.threads1_ns > 0 {
                        thread_scaling
                            .push(100.0 * s.threads4_ns as f64 * 4.0 / s.threads1_ns as f64);
                    }
                    shootdown.push(s.shootdown_ns as f64 / 1000.0);
                }
                _ => {
                    skipped.push((
                        format!("cells on {} CPUs/{}", crate::SMP_CPUS, vmm.id()),
                        format!(
                            "no `rhizo-smp` line ({:?}); output:\n{}",
                            out.result, out.output
                        ),
                    ));
                    break;
                }
            }
        }
        for (name, unit, values) in [
            ("cell.spawn_us", "us", &mut spawn),
            ("cell.run_us", "us", &mut run),
            ("cell.switch_ns", "ns", &mut switch),
            ("cell.switch_untagged_ns", "ns", &mut untagged),
            ("cell.suspend_us", "us", &mut suspend),
            ("cell.resume_us", "us", &mut resume),
            ("cell.resume_eager_us", "us", &mut eager),
            ("cell.resume_touch_us", "us", &mut touch),
            ("cell.fork_us", "us", &mut fork),
            ("cell.fork_touch_us", "us", &mut fork_touch),
            ("cell.spawn_from_template_us", "us", &mut template),
            ("workload.cold_us", "us", &mut cold),
            ("workload.warm_us", "us", &mut warm),
            ("cell.file_write_read_us", "us", &mut file_write_read),
            ("cell.fork_with_files_us", "us", &mut fork_files),
            ("cell.pipe_rtt_ns", "ns", &mut pipe_rtt),
            ("cell.loopback_rtt_us", "us", &mut loopback_rtt),
            ("net.tcp_rtt_us", "us", &mut wire_rtt),
            ("workload.python_cold_us", "us", &mut py_cold),
            ("workload.python_warm_us", "us", &mut py_warm),
            ("workload.cpython_cold_us", "us", &mut cpy_cold),
            ("workload.cpython_warm_us", "us", &mut cpy_warm),
            ("cell.thread_create_us", "us", &mut thread_create),
            ("cell.futex_wake_ns", "ns", &mut futex_wake),
            ("cell.parallel_us", "us", &mut parallel),
            ("cell.scaling_pct", "%", &mut scaling),
            ("cell.ipi_wake_us", "us", &mut ipi_wake),
            ("cell.thread_scaling_pct", "%", &mut thread_scaling),
            ("cell.shootdown_us", "us", &mut shootdown),
        ] {
            if values.is_empty() && name == "net.tcp_rtt_us" {
                let why = match crate::hostnet::for_vmm(vmm) {
                    crate::hostnet::Network::None(why) => why,
                    _ => wire_skipped
                        .clone()
                        .unwrap_or_else(|| "not measured".into()),
                };
                skipped.push((format!("{name}/{}", vmm.id()), why));
            }
            if !values.is_empty() {
                let (median, p90) = summarize(values);
                metrics.push(Metric {
                    name: name.into(),
                    guest: "rhizo".into(),
                    vmm: Some(vmm.id().into()),
                    unit: unit.into(),
                    median,
                    p90,
                    runs: values.len(),
                });
            }
        }
    }
    if let Some(frames) = pt_frames {
        metrics.push(deterministic("cell.pt_frames", "frames", frames as f64));
    }
    if let Some(bytes) = snapshot_bytes {
        metrics.push(deterministic("cell.snapshot_bytes", "bytes", bytes as f64));
    }
    if let Some((idle, suspended)) = density {
        metrics.push(deterministic("cell.idle_bytes", "bytes", idle as f64));
        metrics.push(deterministic(
            "cell.suspended_bytes",
            "bytes",
            suspended as f64,
        ));
    }
    if let Some(frames) = fork_pt_frames {
        metrics.push(deterministic(
            "cell.fork_pt_frames",
            "frames",
            frames as f64,
        ));
    }
    Ok(())
}

/// `(cold_ns, warm_ns)` of the MicroPython line of a `bench-cell` kernel
/// (absent when MicroPython was not built).
fn parse_python_line(output: &str) -> Option<(u64, u64)> {
    let line = output.lines().find(|l| l.starts_with("rhizo-python "))?;
    let field = |key: &str| -> Option<u64> {
        line.split_whitespace()
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?
            .parse()
            .ok()
    };
    Some((field("cold_ns")?, field("warm_ns")?))
}

/// `(cold_ns, warm_ns)` of the CPython line of a `bench-cell` kernel
/// (absent when CPython was not built).
fn parse_cpython_line(output: &str) -> Option<(u64, u64)> {
    let line = output.lines().find(|l| l.starts_with("rhizo-cpython "))?;
    let field = |key: &str| -> Option<u64> {
        line.split_whitespace()
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?
            .parse()
            .ok()
    };
    Some((field("cold_ns")?, field("warm_ns")?))
}

/// `(idle_bytes, suspended_bytes)` of the density line of a `bench-cell`
/// kernel.
fn parse_density_line(output: &str) -> Option<(u64, u64)> {
    let line = output.lines().find(|l| l.starts_with("rhizo-density "))?;
    let field = |key: &str| -> Option<u64> {
        line.split_whitespace()
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?
            .parse()
            .ok()
    };
    Some((field("idle_bytes")?, field("suspended_bytes")?))
}

/// The network card line of a `bench-cell` kernel: the round trip to the
/// host's echo server in nanoseconds, or why it was not measured.
fn parse_wire_line(output: &str) -> Option<std::result::Result<u64, String>> {
    let line = output.lines().find(|l| l.starts_with("rhizo-wire "))?;
    if let Some(why) = line.strip_prefix("rhizo-wire skipped: ") {
        return Some(Err(why.trim().to_string()));
    }
    let value = line.strip_prefix("rhizo-wire tcp_rtt_ns=")?;
    Some(value.trim().parse().map_err(|_| line.to_string()))
}

/// `(pipe_rtt_ns, loopback_rtt_ns)` of the network line of a `bench-cell`
/// kernel.
fn parse_net_line(output: &str) -> Option<(u64, u64)> {
    let line = output.lines().find(|l| l.starts_with("rhizo-net "))?;
    let field = |key: &str| -> Option<u64> {
        line.split_whitespace()
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?
            .parse()
            .ok()
    };
    Some((field("pipe_rtt_ns")?, field("loopback_rtt_ns")?))
}

/// The multi-core line of a `bench-cell` kernel booted with several CPUs.
#[derive(Debug, PartialEq, Eq)]
struct SmpLine {
    cpus: u64,
    serial_ns: u64,
    parallel_ns: u64,
    ipi_wake_ns: u64,
    /// `parsum` with 1 and with 4 threads, and one `munmap` while the
    /// cell's other threads run (design D8 of `add-parallel-threads`).
    threads1_ns: u64,
    threads4_ns: u64,
    shootdown_ns: u64,
}

fn parse_smp_line(output: &str) -> Option<SmpLine> {
    let line = output.lines().find(|l| l.starts_with("rhizo-smp "))?;
    let field = |key: &str| -> Option<u64> {
        line.split_whitespace()
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?
            .parse()
            .ok()
    };
    Some(SmpLine {
        cpus: field("cpus")?,
        serial_ns: field("serial_ns")?,
        parallel_ns: field("parallel_ns")?,
        ipi_wake_ns: field("ipi_wake_ns")?,
        threads1_ns: field("threads1_ns")?,
        threads4_ns: field("threads4_ns")?,
        shootdown_ns: field("shootdown_ns")?,
    })
}

/// `(create_ns, futex_wake_ns)` of the threads line of a `bench-cell`
/// kernel.
fn parse_threads_line(output: &str) -> Option<(u64, u64)> {
    let line = output.lines().find(|l| l.starts_with("rhizo-threads "))?;
    let field = |key: &str| -> Option<u64> {
        line.split_whitespace()
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?
            .parse()
            .ok()
    };
    Some((field("create_ns")?, field("futex_wake_ns")?))
}

/// `(write_read_ns, fork_ns)` of the files line of a `bench-cell` kernel.
fn parse_files_line(output: &str) -> Option<(u64, u64)> {
    let line = output.lines().find(|l| l.starts_with("rhizo-files "))?;
    let field = |key: &str| -> Option<u64> {
        line.split_whitespace()
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?
            .parse()
            .ok()
    };
    Some((field("write_read_ns")?, field("fork_ns")?))
}

/// `(cold_ns, warm_ns)` of the real-workload line of a `bench-cell` kernel.
fn parse_workload_line(output: &str) -> Option<(u64, u64)> {
    let line = output.lines().find(|l| l.starts_with("rhizo-workload "))?;
    let field = |key: &str| -> Option<u64> {
        line.split_whitespace()
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?
            .parse()
            .ok()
    };
    Some((field("cold_ns")?, field("warm_ns")?))
}

/// Fork and template figures printed by a `bench-cell` kernel.
#[derive(Debug, PartialEq, Eq)]
struct ForkLine {
    fork_ns: u64,
    fork_touch_ns: u64,
    fork_pt_frames: u64,
    spawn_from_template_ns: u64,
}

fn parse_fork_line(output: &str) -> Option<ForkLine> {
    let line = output.lines().find(|l| l.starts_with("rhizo-fork "))?;
    let field = |key: &str| -> Option<u64> {
        line.split_whitespace()
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?
            .parse()
            .ok()
    };
    Some(ForkLine {
        fork_ns: field("fork_ns")?,
        fork_touch_ns: field("fork_touch_ns")?,
        fork_pt_frames: field("fork_pt_frames")?,
        spawn_from_template_ns: field("spawn_from_template_ns")?,
    })
}

/// Suspend/resume figures printed by a `bench-cell` kernel.
#[derive(Debug, PartialEq, Eq)]
struct SnapshotLine {
    suspend_ns: u64,
    resume_ns: u64,
    resume_eager_ns: u64,
    resume_touch_ns: u64,
    bytes: u64,
}

fn parse_snapshot_line(output: &str) -> Option<SnapshotLine> {
    let line = output.lines().find(|l| l.starts_with("rhizo-snapshot "))?;
    let field = |key: &str| -> Option<u64> {
        line.split_whitespace()
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?
            .parse()
            .ok()
    };
    Some(SnapshotLine {
        suspend_ns: field("suspend_ns")?,
        resume_ns: field("resume_ns")?,
        resume_eager_ns: field("resume_eager_ns")?,
        resume_touch_ns: field("resume_touch_ns")?,
        bytes: field("bytes")?,
    })
}

/// Figures printed by a `bench-cell` kernel.
#[derive(Debug, PartialEq, Eq)]
struct CellLine {
    spawn_ns: u64,
    run_ns: u64,
    pt_frames: u64,
    switch_ns: u64,
    switch_untagged_ns: u64,
}

fn parse_cell_line(output: &str) -> Option<CellLine> {
    let line = output.lines().find(|l| l.starts_with("rhizo-cell "))?;
    let field = |key: &str| {
        line.split_whitespace()
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?
            .parse()
            .ok()
    };
    Some(CellLine {
        spawn_ns: field("spawn_ns")?,
        run_ns: field("run_ns")?,
        pt_frames: field("pt_frames")?,
        switch_ns: field("switch_ns")?,
        switch_untagged_ns: field("switch_untagged_ns")?,
    })
}

fn deterministic(name: &str, unit: &str, value: f64) -> Metric {
    Metric {
        name: name.into(),
        guest: "rhizo".into(),
        vmm: None,
        unit: unit.into(),
        median: value,
        p90: value,
        runs: 1,
    }
}

struct Samples {
    ready_ms: Vec<f64>,
    cycles: Vec<f64>,
    pt_frames: Option<u64>,
}

impl Samples {
    fn metrics(mut self, guest: &str, vmm: Vmm) -> Vec<Metric> {
        let mut out = Vec::new();
        let mut push = |name: &str, unit: &str, values: &mut Vec<f64>| {
            if !values.is_empty() {
                let (median, p90) = summarize(values);
                out.push(Metric {
                    name: name.into(),
                    guest: guest.into(),
                    vmm: Some(vmm.id().into()),
                    unit: unit.into(),
                    median,
                    p90,
                    runs: values.len(),
                });
            }
        };
        push("boot.time_to_ready_ms", "ms", &mut self.ready_ms);
        push("boot.kernel_cycles", "cycles", &mut self.cycles);
        out
    }
}

/// Boots `guest` `runs` times (after one discarded warm-up run).
fn sample(vmm: Vmm, guest: &Guest, runs: usize, accel: Accel) -> Result<Samples> {
    let name = match guest {
        Guest::Rhizo(_) => "rhizo",
        Guest::ReferenceLinux { .. } => "linux",
    };
    report::text(&format!("{name} on {}: {runs} boots", vmm.id()));
    let opts = RunOptions {
        stop_at_ready: true,
        timeout: RUN_TIMEOUT,
        echo: false,
        accel,
        gdb: false,
        network: false,
        cpus: 1,
    };
    let mut samples = Samples {
        ready_ms: Vec::new(),
        cycles: Vec::new(),
        pt_frames: None,
    };
    for i in 0..=runs {
        let run = vmm::run(vmm, guest, opts)?;
        let Some(ready) = run.time_to_ready else {
            return Err(format!(
                "{name} on {} never printed the ready marker ({:?}); output:\n{}",
                vmm.id(),
                run.result,
                run.output
            ));
        };
        if run.result == VmResult::Failed {
            return Err(format!("{name} on {} failed:\n{}", vmm.id(), run.output));
        }
        if i == 0 {
            samples.pt_frames = parse_pt_frames(&run.output);
            continue; // warm-up: page cache, binary loading
        }
        samples.ready_ms.push(ready.as_secs_f64() * 1000.0);
        if let Some(c) = run.ready_cycles {
            samples.cycles.push(c as f64);
        }
    }
    Ok(samples)
}

fn parse_pt_frames(output: &str) -> Option<u64> {
    let line = output.lines().find(|l| l.starts_with("memory:"))?;
    let before = line.split(" page-table frames").next()?;
    before.rsplit(' ').next()?.parse().ok()
}

fn today() -> String {
    Command::new("date")
        .args(["-u", "+%Y-%m-%d"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn print_table(results: &Results) {
    report::data("bench-results", &results.to_json());
    report::text(&format!(
        "\nbenchmark results (environment `{}`, {}, KVM: {})",
        results.environment,
        results.date,
        if results.kvm { "yes" } else { "no" }
    ));
    report::text(&format!(
        "{:<44} {:>14} {:>14} {:>5}",
        "metric", "median", "p90", "runs"
    ));
    for m in &results.metrics {
        report::text(&format!(
            "{:<44} {:>14} {:>14} {:>5}",
            m.key(),
            format!("{:.3} {}", m.median, m.unit),
            format!("{:.3}", m.p90),
            m.runs
        ));
    }
    for (vmm, reason) in &results.skipped {
        report::text(&format!("skipped {vmm}: {reason}"));
    }
}

// ---------------------------------------------------------------------------
// Baseline
// ---------------------------------------------------------------------------

fn baseline_path() -> PathBuf {
    workspace_root().join(BASELINE)
}

fn load_baseline() -> Result<Value> {
    let path = baseline_path();
    let text =
        fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not valid JSON: {e}", path.display()))
}

/// Whether `metric` is gated in `env`. Metrics listed under `ungated.<env>`
/// are recorded but never fail the gate (for example, TSC cycles on shared CI
/// runners, whose hosts run at different TSC frequencies).
fn is_gated(baseline: &Value, env: &str, metric: &str) -> bool {
    !baseline["ungated"][env]
        .as_array()
        .is_some_and(|names| names.iter().any(|n| n.as_str() == Some(metric)))
}

/// Allowed regression, in percent, for `metric` in `env`.
fn threshold_pct(baseline: &Value, env: &str, metric: &str) -> f64 {
    let t = &baseline["thresholds_pct"];
    if DETERMINISTIC.contains(&metric) {
        t["deterministic"].as_f64().unwrap_or(1.0)
    } else {
        t["timing"][env].as_f64().unwrap_or(50.0)
    }
}

/// A metric that got worse than allowed.
#[derive(Debug, PartialEq)]
pub struct Regression {
    pub key: String,
    pub baseline: f64,
    pub current: f64,
    pub allowed_pct: f64,
}

impl Regression {
    fn worse_pct(&self) -> f64 {
        (self.current / self.baseline - 1.0) * 100.0
    }
}

/// Compares `results` with the stored values for `env` (lower is better).
pub fn regressions(baseline: &Value, env: &str, results: &Results) -> Vec<Regression> {
    let stored = &baseline["environments"][env];
    let mut out = Vec::new();
    for m in results
        .metrics
        .iter()
        .filter(|m| m.guest == "rhizo" && is_gated(baseline, env, &m.name))
    {
        let Some(base) = stored[m.key()].as_f64() else {
            continue;
        };
        let allowed = threshold_pct(baseline, env, &m.name);
        if m.median > base * (1.0 + allowed / 100.0) {
            out.push(Regression {
                key: m.key(),
                baseline: base,
                current: m.median,
                allowed_pct: allowed,
            });
        }
    }
    out
}

fn check_baseline(env: &str, results: &Results) -> Result {
    let baseline = load_baseline()?;
    if baseline["environments"][env]
        .as_object()
        .is_none_or(Map::is_empty)
    {
        report::text(&format!(
            "no baseline stored for environment `{env}`; nothing to compare"
        ));
        return Ok(());
    }
    let found = regressions(&baseline, env, results);
    if found.is_empty() {
        report::text(&format!(
            "benchmark gate: every metric is within its threshold for `{env}`"
        ));
        return Ok(());
    }
    let errors: Vec<String> = found
        .iter()
        .map(|r| {
            Failure::new("bench", format!("benchmark regression: {}", r.key))
                .location(format!("{BASELINE} (environment `{env}`)"))
                .expected(format!(
                    "at most {:.3} (baseline {:.3} + {:.0}%)",
                    r.baseline * (1.0 + r.allowed_pct / 100.0),
                    r.baseline,
                    r.allowed_pct
                ))
                .actual(format!("{:.3} (+{:.1}%)", r.current, r.worse_pct()))
                .hint(format!(
                    "find what made it slower; if the regression is intended, justify it in the \
                     pull request and run `cargo xtask bench --env {env} --update-baseline`"
                ))
                .reproduce(format!("cargo xtask bench --env {env} --check"))
                .into_error()
        })
        .collect();
    Err(errors.join("\n"))
}

fn update_baseline(env: &str, results: &Results) -> Result {
    let mut baseline = load_baseline()?;
    let stored = baseline["environments"]
        .as_object_mut()
        .ok_or("baseline has no `environments` object")?
        .entry(env)
        .or_insert_with(|| json!({}));
    for m in results.metrics.iter().filter(|m| m.guest == "rhizo") {
        stored[m.key()] = json!(m.median);
    }
    let text = serde_json::to_string_pretty(&baseline).expect("serializable");
    fs::write(baseline_path(), text + "\n").map_err(|e| format!("cannot write baseline: {e}"))?;
    report::text(&format!("baseline for `{env}` updated in {BASELINE}"));
    Ok(())
}

// ---------------------------------------------------------------------------
// README chart
// ---------------------------------------------------------------------------

fn vmm_label(id: &str) -> &str {
    match id {
        "qemu-virt" => "QEMU virt",
        "qemu-microvm" => "QEMU microvm",
        "qemu-q35" => "QEMU q35",
        "firecracker" => "Firecracker",
        "cloud-hypervisor" => "Cloud Hypervisor",
        other => other,
    }
}

/// Path of the generated chart, relative to the workspace root.
const CHART: &str = "bench/boot-time.svg";
const CHART_DARK: &str = "bench/boot-time-dark.svg";

/// Order in which VMMs appear in the chart.
const CHART_VMMS: [&str; 4] = [
    "firecracker",
    "cloud-hypervisor",
    "qemu-microvm",
    "qemu-q35",
];

fn median_of<'a>(ready: &[&'a Metric], guest: &str, vmm: &str) -> Option<&'a Metric> {
    ready
        .iter()
        .copied()
        .find(|m| m.guest == guest && m.vmm.as_deref() == Some(vmm))
}

/// Horizontal grouped bar chart (one group per VMM, Rhizo vs Linux), as SVG,
/// for a light or a dark page: no single grey reads at WCAG AA (4.5:1) on
/// both GitHub's white and its #0d1117, so each theme gets its own text.
pub fn chart_svg(results: &Results, dark: bool) -> String {
    const WIDTH: f64 = 760.0;
    const LABEL_W: f64 = 150.0;
    const RIGHT_W: f64 = 150.0;
    const BAR_H: f64 = 16.0;
    const GROUP_H: f64 = 56.0;
    const TOP: f64 = 70.0;
    const RHIZO: &str = "#2563eb";
    const LINUX: &str = "#f97316";
    // 6.2:1 on white, 6.6:1 on #0d1117.
    let text = if dark { "#9ba7b3" } else { "#57606a" };
    // The speed-up label in Rhizo's blue: 5.2:1 on white; on the dark page
    // that blue is 3.7:1, so a lighter one (7.5:1).
    let accent = if dark { "#58a6ff" } else { RHIZO };

    let ready: Vec<&Metric> = results
        .metrics
        .iter()
        .filter(|m| m.name == "boot.time_to_ready_ms")
        .collect();
    let vmms: Vec<&str> = CHART_VMMS
        .iter()
        .copied()
        .filter(|v| {
            median_of(&ready, "rhizo", v).is_some() || median_of(&ready, "linux", v).is_some()
        })
        .collect();
    let max = ready.iter().map(|m| m.median).fold(1.0, f64::max);
    let plot_w = WIDTH - LABEL_W - RIGHT_W;
    let height = TOP + GROUP_H * vmms.len() as f64 + 40.0;

    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{WIDTH}\" height=\"{height}\" \
         viewBox=\"0 0 {WIDTH} {height}\" font-family=\"-apple-system, Segoe UI, Helvetica, Arial, sans-serif\" \
         font-size=\"13\" fill=\"{text}\">\n"
    );
    svg += "  <text x=\"0\" y=\"22\" font-size=\"16\" font-weight=\"600\">Time from VMM launch to guest ready \
         (median, lower is better)</text>\n";
    for (i, (label, color)) in [("Rhizo", RHIZO), ("Linux (reference guest)", LINUX)]
        .iter()
        .enumerate()
    {
        let x = i as f64 * 120.0;
        svg += &format!(
            "  <rect x=\"{x}\" y=\"36\" width=\"12\" height=\"12\" rx=\"2\" fill=\"{color}\"/>\n  \
             <text x=\"{}\" y=\"47\">{label}</text>\n",
            x + 18.0
        );
    }
    for (row, vmm) in vmms.iter().enumerate() {
        let y0 = TOP + row as f64 * GROUP_H;
        svg += &format!(
            "  <text x=\"{}\" y=\"{}\" text-anchor=\"end\" font-weight=\"600\">{}</text>\n",
            LABEL_W - 12.0,
            y0 + BAR_H + 4.0,
            vmm_label(vmm)
        );
        let rhizo = median_of(&ready, "rhizo", vmm);
        let linux = median_of(&ready, "linux", vmm);
        for (k, (metric, color)) in [(rhizo, RHIZO), (linux, LINUX)].into_iter().enumerate() {
            let y = y0 + k as f64 * (BAR_H + 4.0);
            match metric {
                Some(m) => {
                    let w = (m.median / max * plot_w).max(2.0);
                    svg += &format!(
                        "  <rect x=\"{LABEL_W}\" y=\"{y}\" width=\"{w:.1}\" height=\"{BAR_H}\" rx=\"3\" fill=\"{color}\"/>\n  \
                         <text x=\"{:.1}\" y=\"{:.1}\">{:.1} ms</text>\n",
                        LABEL_W + w + 6.0,
                        y + BAR_H - 3.0,
                        m.median
                    );
                }
                None => {
                    svg += &format!(
                        "  <text x=\"{LABEL_W}\" y=\"{:.1}\" font-style=\"italic\">not measured</text>\n",
                        y + BAR_H - 3.0
                    );
                }
            }
        }
        if let (Some(r), Some(l)) = (rhizo, linux) {
            svg += &format!(
                "  <text x=\"{}\" y=\"{}\" text-anchor=\"end\" font-weight=\"600\" fill=\"{accent}\">{:.0}× faster</text>\n",
                WIDTH - 4.0,
                y0 + BAR_H + 4.0,
                l.median / r.median
            );
        }
    }
    svg += &format!(
        "  <text x=\"0\" y=\"{}\" font-size=\"12\">Environment: {}, KVM: {}, {}. \
         Generated by cargo xtask bench --readme.</text>\n</svg>\n",
        height - 10.0,
        results.environment,
        if results.kvm { "yes" } else { "no" },
        results.date
    );
    svg
}

/// Markdown for the README benchmark section.
pub fn readme_section(results: &Results) -> String {
    let ready: Vec<&Metric> = results
        .metrics
        .iter()
        .filter(|m| m.name == "boot.time_to_ready_ms")
        .collect();
    let mut s = String::new();
    s += "Time from launching the VMM to the guest being ready (median, lower is better).\n";
    s += "Rhizo prints its ready line at the end of boot; the reference is Firecracker's\n";
    s += "CI Linux guest kernel with a minimal `/init` that prints the same line, booted\n";
    s += "on the same machine with the same timing method.\n\n";
    if ready.is_empty() {
        s += "_No timing results yet._\n";
        return s;
    }
    s += &format!(
        "<picture><source media=\"(prefers-color-scheme: dark)\" srcset=\"{CHART_DARK}\">\
         <img src=\"{CHART}\" alt=\"Boot time: Rhizo vs a reference Linux guest\"></picture>\n\n"
    );
    s += "| VMM | Rhizo median (ms) | Rhizo p90 (ms) | Linux median (ms) | Linux p90 (ms) | Runs |\n";
    s += "|---|---:|---:|---:|---:|---:|\n";
    let cell = |m: Option<&Metric>, p90: bool| {
        m.map_or_else(
            || "—".to_string(),
            |m| format!("{:.1}", if p90 { m.p90 } else { m.median }),
        )
    };
    for vmm in CHART_VMMS {
        let rhizo = median_of(&ready, "rhizo", vmm);
        let linux = median_of(&ready, "linux", vmm);
        if rhizo.is_none() && linux.is_none() {
            continue;
        }
        s += &format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            vmm_label(vmm),
            cell(rhizo, false),
            cell(rhizo, true),
            cell(linux, false),
            cell(linux, true),
            rhizo.or(linux).map_or(0, |m| m.runs)
        );
    }
    s += &comparison_table(results);
    s += &format!(
        "\nEnvironment: `{}`, KVM: {}, measured {}. Regenerate with\n`cargo xtask bench --readme`.\n",
        results.environment,
        if results.kvm { "yes" } else { "no (emulated)" },
        results.date
    );
    s
}

/// Rhizo cells against existing mechanisms on the same machine (spec
/// `comparisons`): cell figures measured with Rhizo running in Firecracker,
/// next to a Linux guest in Firecracker and Linux processes on the host.
pub fn comparison_table(results: &Results) -> String {
    let find = |name: &str, guest: &str, vmm: &str| {
        results
            .metrics
            .iter()
            .find(|m| m.name == name && m.guest == guest && m.vmm.as_deref() == Some(vmm))
    };
    let time = |m: Option<&Metric>| match m {
        None => "—".to_string(),
        Some(m) => {
            let us = match m.unit.as_str() {
                "ms" => m.median * 1000.0,
                "ns" => m.median / 1000.0,
                _ => m.median,
            };
            if us >= 1000.0 {
                format!("{:.2} ms", us / 1000.0)
            } else {
                format!("{us:.1} µs")
            }
        }
    };
    let cell = |name: &str| time(find(name, "rhizo", "firecracker"));
    let process = |name: &str, vmm: &str| time(find(name, "linux-process", vmm));
    let criu = find("snapshot.restore_ms", "criu", "host");
    // The same operation by a cell and by a Linux process, in the same kind of
    // VM (design D3 of `add-fair-comparisons`).
    let fair = [
        (
            "The `fork` call, holding 50 MiB written",
            cell("cell.fork_us"),
            process("process.fork_call_us", "firecracker"),
            process("process.fork_call_us", "host"),
        ),
        (
            "Fork, then the child writes all 50 MiB and exits",
            cell("cell.fork_touch_us"),
            process("process.fork_touch_us", "firecracker"),
            process("process.fork_touch_us", "host"),
        ),
        (
            "Start a program and run it to its exit (`hello`)",
            cell("cell.run_us"),
            process("process.spawn_us", "firecracker"),
            process("process.spawn_us", "host"),
        ),
        (
            "Resume a suspended 8 MiB program: lazy / eager (CRIU on the host)",
            format!(
                "{} / {}",
                cell("cell.resume_us"),
                cell("cell.resume_eager_us")
            ),
            "—".into(),
            criu.map_or("—".into(), |m| format!("{} (CRIU)", time(Some(m)))),
        ),
    ];
    // Whole VMs and sandboxes: a larger scope than a cell, shown for scale.
    let whole = [
        (
            "Start a program and run it to its exit (`hello`)",
            cell("cell.run_us"),
            time(find("boot.time_to_ready_ms", "linux", "firecracker")),
            time(find("sandbox.run_ms", "gvisor", "runsc")),
        ),
        (
            "Resume a suspended environment until it runs (eager for the cell)",
            cell("cell.resume_eager_us"),
            time(find("snapshot.restore_ms", "linux", "firecracker")),
            time(find("snapshot.restore_ms", "gvisor", "runsc")),
        ),
    ];
    if fair
        .iter()
        .chain(&whole)
        .all(|r| r.1 == "—" || r.1 == "— / —")
    {
        return String::new();
    }
    let mut s = String::from(
        "\n### A cell against a Linux process, in the same kind of VM\n\n\
         Same machine, same run. Rhizo runs inside Firecracker, and so does the Linux \
         guest of the middle column (the same static programs, as its `/init`): both \
         sides pay the same virtualization. The last column is the same program on the \
         host, one layer of virtualization less. Lazy resume brings pages back on first \
         touch; eager restores every page before the program runs.\n\n\
         | Operation | Rhizo cell | Linux process, same VM | Linux process, host |\n\
         |---|---:|---:|---:|\n",
    );
    for (what, rhizo, vm, host) in fair {
        s += &format!("| {what} | {rhizo} | {vm} | {host} |\n");
    }
    s += "\n### Whole VMs and sandboxes, for scale\n\n\
          A microVM or a sandbox starts or restores a whole environment (a VM, or a \
          sandbox process with its own kernel), a larger scope than a cell: these \
          figures show the scale, not a like-for-like speed-up. The microVM is a Linux \
          guest in Firecracker (boot to its first output, or a snapshot restored by a new \
          Firecracker process); the gVisor sandbox runs on the host.\n\n\
          | Operation | Rhizo cell | Firecracker microVM | gVisor sandbox |\n\
          |---|---:|---:|---:|\n";
    for (what, rhizo, firecracker, gvisor) in whole {
        s += &format!("| {what} | {rhizo} | {firecracker} | {gvisor} |\n");
    }
    let (cold, warm) = (
        find("workload.python_cold_us", "rhizo", "firecracker"),
        find("workload.python_warm_us", "rhizo", "firecracker"),
    );
    let cpython = |guest: &str| {
        (
            find("workload.cpython_cold_us", guest, "firecracker"),
            find("workload.cpython_warm_us", guest, "firecracker"),
        )
    };
    let (cpy_cold, cpy_warm) = cpython("rhizo");
    let (linux_cold, linux_warm) = cpython("linux-process");
    if (cold.is_some() && warm.is_some()) || (cpy_cold.is_some() && cpy_warm.is_some()) {
        s += "\n### A real interpreter\n";
    }
    if let (Some(c), Some(w)) = (cpy_cold, cpy_warm) {
        s += &format!(
            "\nCPython (static, 3.13.18) answering one agent tool call \
             (`cells/python/agent_demo.py`: JSON, regular expressions, dates; the standard \
             library it imports compiled in the cell's own files), in a cell and as a Linux \
             process in the same Firecracker VM:\n\n\
             | Started | Rhizo cell | Linux process |\n|---|---:|---:|\n\
             | Cold, from its executable (interpreter start, imports, warm-up) | {} | {} |\n\
             | Forked from an interpreter frozen (cell) or kept (process) after the warm-up \
             | {} ({:.1}× faster) | {} |\n",
            time(cpy_cold),
            time(linux_cold),
            time(cpy_warm),
            c.median / w.median,
            time(linux_warm)
        );
    }
    if let (Some(c), Some(w)) = (cold, warm) {
        s += &format!(
            "\nMicroPython (static, v1.24.1) answering one request \
             (`cells/python/rhizo_demo.py`, imported from the cell's own files), in a cell \
             in Firecracker:\n\n\
             | Started | Time |\n|---|---:|\n\
             | Cold, from its executable (interpreter start, imports, warm-up) | {} |\n\
             | Forked from a template frozen after the warm-up | {} ({:.1}× faster) |\n",
            time(cold),
            time(warm),
            c.median / w.median
        );
    }
    s
}

fn update_readme(results: &Results) -> Result {
    let path = workspace_root().join("README.md");
    let text = fs::read_to_string(&path).map_err(|e| format!("cannot read README.md: {e}"))?;
    let new = replace_section(&text, &readme_section(results))?;
    fs::write(&path, new).map_err(|e| format!("cannot write README.md: {e}"))?;
    for (file, dark) in [(CHART, false), (CHART_DARK, true)] {
        let chart = workspace_root().join(file);
        fs::write(&chart, chart_svg(results, dark))
            .map_err(|e| format!("cannot write {}: {e}", chart.display()))?;
    }
    report::text("README.md benchmark section updated");
    Ok(())
}

fn replace_section(text: &str, body: &str) -> Result<String> {
    let start = text
        .find(README_START)
        .ok_or("README.md has no `<!-- bench:start -->` marker")?;
    let end = text
        .find(README_END)
        .ok_or("README.md has no `<!-- bench:end -->` marker")?;
    Ok(format!(
        "{}{README_START}\n{body}{}",
        &text[..start],
        &text[end..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_table_puts_each_mechanism_in_its_column() {
        let mut fork = metric("cell.fork_us", "rhizo", Some("firecracker"), 1.5);
        fork.unit = "us".into();
        let mut process = metric(
            "process.fork_call_us",
            "linux-process",
            Some("host"),
            1700.0,
        );
        process.unit = "us".into();
        let mut in_vm = metric(
            "process.fork_call_us",
            "linux-process",
            Some("firecracker"),
            0.6,
        );
        in_vm.unit = "us".into();
        let results = Results {
            environment: "ci".into(),
            date: "today".into(),
            kvm: true,
            metrics: vec![
                fork,
                process,
                in_vm,
                metric("snapshot.restore_ms", "criu", Some("host"), 45.0),
                metric("snapshot.restore_ms", "linux", Some("firecracker"), 12.5),
                metric("snapshot.restore_ms", "gvisor", Some("runsc"), 120.0),
            ],
            skipped: Vec::new(),
        };
        let table = comparison_table(&results);
        assert!(
            table.contains(
                "| The `fork` call, holding 50 MiB written | 1.5 µs | 0.6 µs | 1.70 ms |"
            ),
            "{table}"
        );
        assert!(
            table.contains(
                "| Resume a suspended environment until it runs (eager for the cell) | — | \
                 12.50 ms | 120.00 ms |"
            ),
            "{table}"
        );
        assert!(table.contains("| — / — | — | 45.00 ms (CRIU) |"), "{table}");
        let empty = Results {
            metrics: Vec::new(),
            ..results
        };
        assert_eq!(comparison_table(&empty), "", "no cell figures, no table");
    }

    fn metric(name: &str, guest: &str, vmm: Option<&str>, median: f64) -> Metric {
        Metric {
            name: name.into(),
            guest: guest.into(),
            vmm: vmm.map(Into::into),
            unit: "ms".into(),
            median,
            p90: median,
            runs: 5,
        }
    }

    fn results(metrics: Vec<Metric>) -> Results {
        Results {
            environment: "ci".into(),
            date: "2026-11-19".into(),
            kvm: true,
            metrics,
            skipped: vec![],
        }
    }

    #[test]
    fn median_and_p90() {
        assert_eq!(summarize(&mut [3.0, 1.0, 2.0]), (2.0, 3.0));
        assert_eq!(summarize(&mut [4.0, 1.0, 3.0, 2.0]), (2.5, 4.0));
        let mut ten: Vec<f64> = (1..=10).map(f64::from).collect();
        assert_eq!(summarize(&mut ten), (5.5, 9.0));
    }

    #[test]
    fn regression_gate_uses_thresholds_per_kind() {
        let baseline = json!({
            "thresholds_pct": {"deterministic": 1, "timing": {"ci": 50, "dedicated": 5}},
            "environments": {"ci": {
                "boot.time_to_ready_ms/rhizo/qemu-microvm": 10.0,
                "image.size_bytes": 1000.0
            }}
        });
        let ok = results(vec![
            metric("boot.time_to_ready_ms", "rhizo", Some("qemu-microvm"), 14.9),
            metric("image.size_bytes", "rhizo", None, 1010.0),
        ]);
        assert!(regressions(&baseline, "ci", &ok).is_empty());

        let slow = results(vec![
            metric("boot.time_to_ready_ms", "rhizo", Some("qemu-microvm"), 15.1),
            metric("image.size_bytes", "rhizo", None, 1011.0),
            metric(
                "boot.time_to_ready_ms",
                "linux",
                Some("qemu-microvm"),
                999.0,
            ),
        ]);
        let found = regressions(&baseline, "ci", &slow);
        let keys: Vec<_> = found.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "boot.time_to_ready_ms/rhizo/qemu-microvm",
                "image.size_bytes"
            ]
        );
    }

    #[test]
    fn ungated_metrics_never_fail_the_gate() {
        let baseline = json!({
            "thresholds_pct": {"deterministic": 1, "timing": {"ci": 100}},
            "ungated": {"ci": ["boot.kernel_cycles"]},
            "environments": {"ci": {
                "boot.kernel_cycles/rhizo/qemu-q35": 100.0,
                "boot.time_to_ready_ms/rhizo/qemu-q35": 10.0
            }}
        });
        let r = results(vec![
            metric("boot.kernel_cycles", "rhizo", Some("qemu-q35"), 1000.0),
            metric("boot.time_to_ready_ms", "rhizo", Some("qemu-q35"), 20.5),
        ]);
        let keys: Vec<_> = regressions(&baseline, "ci", &r)
            .into_iter()
            .map(|r| r.key)
            .collect();
        assert_eq!(keys, ["boot.time_to_ready_ms/rhizo/qemu-q35"]);
        assert!(
            regressions(&baseline, "dedicated", &r).is_empty(),
            "no baseline there"
        );
    }

    #[test]
    fn results_round_trip_through_json() {
        let r = results(vec![metric(
            "boot.kernel_cycles",
            "rhizo",
            Some("firecracker"),
            5.0,
        )]);
        let back = Results::from_json(&r.to_json()).unwrap();
        assert_eq!(back.metrics, r.metrics);
    }

    #[test]
    fn readme_section_has_chart_table_and_environment() {
        let r = results(vec![
            metric("boot.time_to_ready_ms", "rhizo", Some("firecracker"), 4.2),
            metric("boot.time_to_ready_ms", "linux", Some("firecracker"), 130.0),
            metric("boot.time_to_ready_ms", "rhizo", Some("qemu-microvm"), 40.0),
        ]);
        let s = readme_section(&r);
        assert!(s.contains("srcset=\"bench/boot-time-dark.svg\""));
        assert!(s.contains(
            "<img src=\"bench/boot-time.svg\" alt=\"Boot time: Rhizo vs a reference Linux guest\">"
        ));
        assert!(s.contains("| Firecracker | 4.2 | 4.2 | 130.0 | 130.0 | 5 |"));
        assert!(s.contains("| QEMU microvm | 40.0 | 40.0 | — | — | 5 |"));
        assert!(s.contains("Environment: `ci`, KVM: yes, measured 2026-11-19"));
    }

    #[test]
    fn chart_has_two_colors_legend_values_and_speedup() {
        let r = results(vec![
            metric("boot.time_to_ready_ms", "rhizo", Some("firecracker"), 4.0),
            metric("boot.time_to_ready_ms", "linux", Some("firecracker"), 120.0),
            metric("boot.time_to_ready_ms", "rhizo", Some("qemu-microvm"), 40.0),
        ]);
        let svg = chart_svg(&r, false);
        assert!(svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"));
        assert!(svg.contains("#2563eb") && svg.contains("#f97316"));
        assert!(svg.contains(">Rhizo</text>") && svg.contains(">Linux (reference guest)</text>"));
        assert!(svg.contains(">4.0 ms</text>") && svg.contains(">120.0 ms</text>"));
        assert!(svg.contains(">30× faster</text>"));
        assert!(
            svg.contains("not measured"),
            "missing Linux on microvm is labelled"
        );
        // One label per VMM row, never squeezed under the bars.
        assert_eq!(
            svg.matches("text-anchor=\"end\" font-weight=\"600\">")
                .count(),
            2
        );
    }

    #[test]
    fn readme_markers_are_required_and_content_replaced() {
        let text = "a\n<!-- bench:start -->\nold\n<!-- bench:end -->\nb";
        assert_eq!(
            replace_section(text, "new\n").unwrap(),
            "a\n<!-- bench:start -->\nnew\n<!-- bench:end -->\nb"
        );
        assert!(replace_section("no markers", "x").is_err());
    }

    #[test]
    fn page_table_frames_are_parsed() {
        assert_eq!(
            parse_pt_frames("x\nmemory: 126 MiB usable, 126 MiB free, 5 page-table frames\n"),
            Some(5)
        );
        assert_eq!(parse_pt_frames("memory: 126 MiB usable"), None);
    }

    #[test]
    fn options_parse() {
        let args: Vec<String> = ["--runs", "5", "--vmm", "q35,fc", "--env", "ci", "--check"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let o = Options::parse(&args).unwrap();
        assert_eq!(o.runs, 5);
        assert_eq!(o.vmms, vec![Vmm::QemuQ35, Vmm::Firecracker]);
        assert!(o.check && o.env == "ci");
        assert!(Options::parse(&["--runs".to_string(), "0".to_string()]).is_err());
    }

    #[test]
    fn workload_line_is_parsed() {
        assert_eq!(
            parse_workload_line("x\nrhizo-workload cold_ns=900 warm_ns=30\n"),
            Some((900, 30))
        );
    }

    #[test]
    fn comparison_table_shows_python_when_measured() {
        let mut m = metric(
            "workload.python_cold_us",
            "rhizo",
            Some("firecracker"),
            3600.0,
        );
        m.unit = "us".into();
        let mut w = metric(
            "workload.python_warm_us",
            "rhizo",
            Some("firecracker"),
            540.0,
        );
        w.unit = "us".into();
        let mut run = metric("cell.run_us", "rhizo", Some("firecracker"), 7.2);
        run.unit = "us".into();
        let t = comparison_table(&results(vec![run.clone(), m, w]));
        assert!(t.contains(
            "| Cold, from its executable (interpreter start, imports, warm-up) | 3.60 ms |"
        ));
        assert!(t.contains("| 540.0 µs (6.7× faster) |"), "{t}");
        assert!(!comparison_table(&results(vec![run])).contains("A real interpreter"));
    }

    #[test]
    fn comparison_table_shows_cpython_next_to_linux() {
        let us = |name: &str, guest: &str, v: f64| {
            let mut m = metric(name, guest, Some("firecracker"), v);
            m.unit = "us".into();
            m
        };
        let run = us("cell.run_us", "rhizo", 7.2);
        let t = comparison_table(&results(vec![
            run.clone(),
            us("workload.cpython_cold_us", "rhizo", 30000.0),
            us("workload.cpython_warm_us", "rhizo", 1500.0),
            us("workload.cpython_cold_us", "linux-process", 40000.0),
            us("workload.cpython_warm_us", "linux-process", 3000.0),
        ]));
        assert!(t.contains("### A real interpreter"), "{t}");
        assert!(t.contains("| Cold, from its executable (interpreter start, imports, warm-up) | 30.00 ms | 40.00 ms |"), "{t}");
        assert!(t.contains("| 1.50 ms (20.0× faster) | 3.00 ms |"), "{t}");
        assert!(!t.contains("MicroPython"));
        let only_cell = comparison_table(&results(vec![
            run,
            us("workload.cpython_cold_us", "rhizo", 30000.0),
            us("workload.cpython_warm_us", "rhizo", 1500.0),
        ]));
        assert!(only_cell.contains("| 30.00 ms | — |"), "{only_cell}");
    }

    #[test]
    fn python_line_is_parsed() {
        assert_eq!(
            parse_python_line("rhizo-python cold_ns=900000 warm_ns=40000\n"),
            Some((900_000, 40_000))
        );
        assert_eq!(parse_python_line("no python here"), None);
    }

    #[test]
    fn cpython_line_is_parsed() {
        let out =
            "rhizo-python cold_ns=1 warm_ns=2\nrhizo-cpython cold_ns=30000000 warm_ns=900000\n";
        assert_eq!(parse_cpython_line(out), Some((30_000_000, 900_000)));
        assert_eq!(parse_cpython_line("rhizo-python cold_ns=1 warm_ns=2"), None);
    }

    #[test]
    fn density_line_is_parsed() {
        assert_eq!(
            parse_density_line("rhizo-density idle_bytes=20480 suspended_bytes=40000\n"),
            Some((20_480, 40_000))
        );
        assert_eq!(parse_density_line("rhizo-density idle_bytes=1\n"), None);
    }

    #[test]
    fn files_line_is_parsed() {
        assert_eq!(
            parse_files_line("rhizo-files write_read_ns=120000 fork_ns=1500\n"),
            Some((120_000, 1500))
        );
        assert_eq!(parse_files_line("rhizo-files write_read_ns=1\n"), None);
    }

    #[test]
    fn wire_line_is_parsed() {
        assert_eq!(
            parse_wire_line("rhizo-wire tcp_rtt_ns=52000\n"),
            Some(Ok(52_000))
        );
        assert_eq!(
            parse_wire_line("rhizo-wire skipped: no card\n"),
            Some(Err("no card".into()))
        );
        assert_eq!(parse_wire_line("nothing\n"), None);
    }

    #[test]
    fn net_line_is_parsed() {
        assert_eq!(
            parse_net_line("x\nrhizo-net pipe_rtt_ns=310 loopback_rtt_ns=4200\n"),
            Some((310, 4200))
        );
        assert_eq!(parse_net_line("rhizo-net pipe_rtt_ns=1\n"), None);
    }

    #[test]
    fn threads_line_is_parsed() {
        assert_eq!(
            parse_threads_line("x\nrhizo-threads create_ns=4200 futex_wake_ns=650\n"),
            Some((4200, 650))
        );
        assert_eq!(parse_threads_line("rhizo-threads create_ns=1\n"), None);
    }

    #[test]
    fn smp_line_is_parsed() {
        assert_eq!(
            parse_smp_line(
                "cpus: 4 online\nrhizo-smp cpus=4 serial_ns=800 parallel_ns=220 ipi_wake_ns=9000 \
                 threads1_ns=400 threads4_ns=110 shootdown_ns=3000\n"
            ),
            Some(SmpLine {
                cpus: 4,
                serial_ns: 800,
                parallel_ns: 220,
                ipi_wake_ns: 9000,
                threads1_ns: 400,
                threads4_ns: 110,
                shootdown_ns: 3000,
            })
        );
        assert_eq!(parse_smp_line("rhizo-smp cpus=4 serial_ns=1\n"), None);
    }

    #[test]
    fn fork_line_is_parsed() {
        let out = "rhizo-fork fork_ns=1 fork_touch_ns=2 fork_pt_frames=1 spawn_from_template_ns=4";
        assert_eq!(
            parse_fork_line(out),
            Some(ForkLine {
                fork_ns: 1,
                fork_touch_ns: 2,
                fork_pt_frames: 1,
                spawn_from_template_ns: 4,
            })
        );
    }

    #[test]
    fn snapshot_line_is_parsed() {
        let out =
            "rhizo-snapshot suspend_ns=1 resume_ns=2 resume_eager_ns=3 resume_touch_ns=4 bytes=5";
        assert_eq!(
            parse_snapshot_line(out),
            Some(SnapshotLine {
                suspend_ns: 1,
                resume_ns: 2,
                resume_eager_ns: 3,
                resume_touch_ns: 4,
                bytes: 5,
            })
        );
    }

    #[test]
    fn cell_line_is_parsed() {
        let out = "rhizo-ready cycles=1\nrhizo-cell spawn_ns=12000 run_ns=30000 pt_frames=7 \
                   switch_ns=300 switch_untagged_ns=900\n";
        assert_eq!(
            parse_cell_line(out),
            Some(CellLine {
                spawn_ns: 12000,
                run_ns: 30000,
                pt_frames: 7,
                switch_ns: 300,
                switch_untagged_ns: 900,
            })
        );
        assert_eq!(parse_cell_line("rhizo-cell spawn_ns=x"), None);
    }
}
