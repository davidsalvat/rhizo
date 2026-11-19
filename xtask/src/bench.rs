//! `cargo xtask bench`: boot benchmarks, baseline gate and README chart
//! (design decisions D7 and D8).

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
const DETERMINISTIC: [&str; 2] = ["image.size_bytes", "memory.kernel_pt_frames"];

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
            vmms: Vmm::ALL.to_vec(),
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
        println!("results written to {}", path.display());
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
            println!("skipping {}: {reason}", vmm.id());
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
    Ok(Results {
        environment: opts.env.clone(),
        date: today(),
        kvm: vmm::kvm_available(),
        metrics,
        skipped,
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
    print!("{name} on {}: ", vmm.id());
    let opts = RunOptions {
        stop_at_ready: true,
        timeout: RUN_TIMEOUT,
        echo: false,
        accel,
        gdb: false,
    };
    let mut samples = Samples {
        ready_ms: Vec::new(),
        cycles: Vec::new(),
        pt_frames: None,
    };
    for i in 0..=runs {
        let run = vmm::run(vmm, guest, opts)?;
        let Some(ready) = run.time_to_ready else {
            println!("failed");
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
        print!(".");
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }
    println!(" done");
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
    println!(
        "\nbenchmark results (environment `{}`, {}, KVM: {})",
        results.environment,
        results.date,
        if results.kvm { "yes" } else { "no" }
    );
    println!(
        "{:<44} {:>14} {:>14} {:>5}",
        "metric", "median", "p90", "runs"
    );
    for m in &results.metrics {
        println!(
            "{:<44} {:>14} {:>14} {:>5}",
            m.key(),
            format!("{:.3} {}", m.median, m.unit),
            format!("{:.3}", m.p90),
            m.runs
        );
    }
    for (vmm, reason) in &results.skipped {
        println!("skipped {vmm}: {reason}");
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
        println!("no baseline stored for environment `{env}`; nothing to compare");
        return Ok(());
    }
    let found = regressions(&baseline, env, results);
    if found.is_empty() {
        println!("benchmark gate: every metric is within its threshold for `{env}`");
        return Ok(());
    }
    let mut message = format!("benchmark regression in environment `{env}`:\n");
    for r in &found {
        message += &format!(
            "  {}: {:.3} -> {:.3} (+{:.1}%, allowed +{:.0}%)\n",
            r.key,
            r.baseline,
            r.current,
            r.worse_pct(),
            r.allowed_pct
        );
    }
    message += "If the regression is intended, justify it in the pull request and run\n";
    message += &format!("  cargo xtask bench --env {env} --update-baseline");
    Err(message)
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
    println!("baseline for `{env}` updated in {BASELINE}");
    Ok(())
}

// ---------------------------------------------------------------------------
// README chart
// ---------------------------------------------------------------------------

fn vmm_label(id: &str) -> &str {
    match id {
        "qemu-microvm" => "QEMU microvm",
        "qemu-q35" => "QEMU q35",
        "firecracker" => "Firecracker",
        "cloud-hypervisor" => "Cloud Hypervisor",
        other => other,
    }
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
    let labels: Vec<String> = ready
        .iter()
        .map(|m| {
            let guest = if m.guest == "rhizo" { "Rhizo" } else { "Linux" };
            format!(
                "\"{} · {}\"",
                guest,
                vmm_label(m.vmm.as_deref().unwrap_or("?"))
            )
        })
        .collect();
    let values: Vec<String> = ready.iter().map(|m| format!("{:.1}", m.median)).collect();
    s += "```mermaid\nxychart-beta\n";
    s += "    title \"Time to ready (ms, median, lower is better)\"\n";
    s += &format!("    x-axis [{}]\n", labels.join(", "));
    s += "    y-axis \"milliseconds\"\n";
    s += &format!("    bar [{}]\n```\n\n", values.join(", "));
    s += "| Guest | VMM | Median (ms) | p90 (ms) | Runs |\n|---|---|---:|---:|---:|\n";
    for m in &ready {
        s += &format!(
            "| {} | {} | {:.1} | {:.1} | {} |\n",
            if m.guest == "rhizo" {
                "Rhizo"
            } else {
                "Linux (reference)"
            },
            vmm_label(m.vmm.as_deref().unwrap_or("?")),
            m.median,
            m.p90,
            m.runs
        );
    }
    s += &format!(
        "\nEnvironment: `{}`, KVM: {}, measured {}. Regenerate with\n`cargo xtask bench --readme`.\n",
        results.environment,
        if results.kvm { "yes" } else { "no (emulated)" },
        results.date
    );
    s
}

fn update_readme(results: &Results) -> Result {
    let path = workspace_root().join("README.md");
    let text = fs::read_to_string(&path).map_err(|e| format!("cannot read README.md: {e}"))?;
    let new = replace_section(&text, &readme_section(results))?;
    fs::write(&path, new).map_err(|e| format!("cannot write README.md: {e}"))?;
    println!("README.md benchmark section updated");
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
        ]);
        let s = readme_section(&r);
        assert!(s.contains("xychart-beta"));
        assert!(s.contains("\"Rhizo · Firecracker\", \"Linux · Firecracker\""));
        assert!(s.contains("bar [4.2, 130.0]"));
        assert!(s.contains("| Linux (reference) | Firecracker | 130.0 |"));
        assert!(s.contains("Environment: `ci`, KVM: yes, measured 2026-11-19"));
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
}
