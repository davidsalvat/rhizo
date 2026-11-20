//! Where the kernel's time goes (spec `profiling`): the breakdowns a
//! `bench-cell` kernel built with `profile` prints, and the benchmarks
//! counted in retired instructions and cycles (a PMU's passes, or QEMU's
//! instruction counting).

use crate::bench::{Metric, Options, RUN_TIMEOUT, summarize};
use crate::vmm::{self, Accel, Guest, RunOptions, VmResult, Vmm};
use crate::{Profile, Result, build_kernel, report};
use std::collections::BTreeMap;

/// Each figure of a `bench-cell` run measured with the benchmarks' clock:
/// `(line, field, metric name without its unit)`. A pass of the PMU, or a
/// run under `-icount`, reports these as counts.
pub const COUNTED: &[(&str, &str, &str)] = &[
    ("rhizo-cell", "spawn_ns", "cell.spawn"),
    ("rhizo-cell", "run_ns", "cell.run"),
    ("rhizo-cell", "switch_ns", "cell.switch"),
    ("rhizo-cell", "switch_untagged_ns", "cell.switch_untagged"),
    ("rhizo-fork", "fork_ns", "cell.fork"),
    ("rhizo-fork", "fork_touch_ns", "cell.fork_touch"),
    (
        "rhizo-fork",
        "spawn_from_template_ns",
        "cell.spawn_from_template",
    ),
    ("rhizo-workload", "cold_ns", "workload.cold"),
    ("rhizo-workload", "warm_ns", "workload.warm"),
    ("rhizo-python", "cold_ns", "workload.python_cold"),
    ("rhizo-python", "warm_ns", "workload.python_warm"),
    ("rhizo-cpython", "cold_ns", "workload.cpython_cold"),
    ("rhizo-cpython", "warm_ns", "workload.cpython_warm"),
    ("rhizo-files", "write_read_ns", "cell.file_write_read"),
    ("rhizo-files", "fork_ns", "cell.fork_with_files"),
    ("rhizo-net", "pipe_rtt_ns", "cell.pipe_rtt"),
    ("rhizo-net", "loopback_rtt_ns", "cell.loopback_rtt"),
    ("rhizo-net", "tcp_resume_ns", "cell.tcp_resume"),
    ("rhizo-snapshot", "suspend_ns", "cell.suspend"),
    ("rhizo-snapshot", "resume_ns", "cell.resume"),
    ("rhizo-snapshot", "resume_eager_ns", "cell.resume_eager"),
    ("rhizo-snapshot", "resume_touch_ns", "cell.resume_touch"),
    ("rhizo-ops", "syscall_ns", "cell.syscall"),
    ("rhizo-ops", "cow_fault_ns", "cell.cow_fault"),
];

/// The value of `key=` on the first line of `text` starting with `prefix`
/// and a space.
pub fn field(text: &str, prefix: &str, key: &str) -> Option<u64> {
    text.lines()
        .find(|l| l.strip_prefix(prefix).is_some_and(|r| r.starts_with(' ')))?
        .split_whitespace()
        .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?
        .parse()
        .ok()
}

/// The [`COUNTED`] figures of `text` (one pass of a run), named with
/// `suffix` (`insns` or `cycles`).
pub fn counted(text: &str, suffix: &str) -> Vec<(String, f64)> {
    COUNTED
        .iter()
        .filter_map(|&(line, key, name)| {
            Some((format!("{name}_{suffix}"), field(text, line, key)? as f64))
        })
        .collect()
}

/// The PMU passes of a `bench-cell` run (`rhizo-pass instructions`, then
/// `rhizo-pass cycles`): their [`COUNTED`] figures as `_insns` and
/// `_cycles` metrics.
pub fn pmu_passes(output: &str) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let passes = output.split("\nrhizo-pass ").skip(1);
    for pass in passes {
        let (name, text) = pass.split_once('\n').unwrap_or((pass, ""));
        let suffix = match name.trim() {
            "instructions" => "insns",
            "cycles" => "cycles",
            _ => continue,
        };
        out.extend(counted(text, suffix));
    }
    out
}

/// What the kernel found to count with: `rhizo-pmu …` without its prefix.
pub fn pmu_line(output: &str) -> Option<&str> {
    output
        .lines()
        .find_map(|l| l.strip_prefix("rhizo-pmu "))
        .map(str::trim)
}

/// The breakdowns of a profile run, as `(breakdown.<workload>.<part>, ns)`,
/// with the cost of a copy-on-write fault (`cell.cow_fault`), and how often
/// each lock made a CPU wait (`lock.<name>_waits`).
pub fn breakdowns(output: &str) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    for line in output.lines() {
        let Some(rest) = line.strip_prefix("rhizo-breakdown ") else {
            if let Some(locks) = line.strip_prefix("rhizo-locks ") {
                for kv in locks.split_whitespace() {
                    let Some((name, value)) = kv.split_once('=') else {
                        continue;
                    };
                    let waits = value.split('/').next().and_then(|w| w.parse().ok());
                    if let Some(waits) = waits {
                        out.push((format!("lock.{name}_waits"), waits));
                    }
                }
            }
            continue;
        };
        let mut words = rest.split_whitespace();
        let Some(workload) = words.next() else {
            continue;
        };
        for kv in words {
            let Some((part, value)) = kv.split_once("_ns=") else {
                continue;
            };
            if let Ok(ns) = value.parse::<f64>() {
                out.push((format!("breakdown.{workload}.{part}"), ns));
            }
        }
    }
    out
}

/// Runs the `profile` kernel on every available VMM for the breakdowns
/// (with one CPU, then with several), and both kernels under QEMU with
/// `-icount` for the figures counted in instructions (spec `profiling`).
pub fn measure(
    opts: &Options,
    metrics: &mut Vec<Metric>,
    skipped: &mut Vec<(String, String)>,
) -> Result {
    // Kept apart: the next kernel build writes the same path.
    let built = build_kernel(Profile::Release, &["bench-cell", "profile"])?;
    let mut name = built.file_stem().unwrap_or_default().to_os_string();
    name.push("-profile");
    let kept = built.with_file_name(name);
    let kept = match built.extension() {
        Some(ext) => kept.with_extension(ext),
        None => kept,
    };
    std::fs::copy(&built, &kept).map_err(|e| format!("cannot copy {}: {e}", built.display()))?;
    let profiled = Guest::Rhizo(kept);
    let one = RunOptions {
        stop_at_ready: false,
        timeout: RUN_TIMEOUT,
        echo: false,
        accel: opts.accel,
        gdb: false,
        network: false,
        pci_card: false,
        cpus: 1,
        memory_mib: vmm::DEFAULT_MEMORY_MIB,
        bench: None,
    };
    for &vmm in &opts.vmms {
        if vmm.unavailable_reason(opts.accel).is_some() {
            continue; // already reported by the boot benchmark
        }
        report::text(&format!("breakdowns on {}", vmm.id()));
        let mut samples = Samples::default();
        for cpus in [1, crate::SMP_CPUS] {
            for _ in 0..opts.runs.clamp(1, 3) {
                let out = vmm::run(vmm, &profiled, RunOptions { cpus, ..one })?;
                if out.result == VmResult::Failed || !out.output.contains("rhizo-breakdown ") {
                    skipped.push((
                        format!("breakdowns on {cpus} CPUs/{}", vmm.id()),
                        format!("no breakdown ({:?}); output:\n{}", out.result, out.output),
                    ));
                    break;
                }
                for (name, ns) in breakdowns(&out.output) {
                    match name.strip_suffix("_waits") {
                        Some(_) => samples.add(name, "waits", ns),
                        None => samples.add(format!("{name}_us"), "us", ns / 1000.0),
                    }
                }
                if let Some(ns) = field(&out.output, "rhizo-ops", "cow_fault_ns") {
                    samples.add("cell.cow_fault_ns".into(), "ns", ns as f64);
                }
            }
        }
        metrics.extend(samples.metrics(vmm.id()));
    }
    if opts.icount {
        icount(metrics, skipped, &profiled)?;
    }
    Ok(())
}

/// The benchmarks and breakdowns under QEMU with `-icount`, once each: the
/// guest's clock counts its instructions, so every figure is a count of
/// retired instructions, the same on every run and every host (spec
/// `profiling`).
fn icount(
    metrics: &mut Vec<Metric>,
    skipped: &mut Vec<(String, String)>,
    profiled: &Guest,
) -> Result {
    let vmm = Vmm::default_for(crate::arch::current());
    let label = format!("{}-icount", vmm.id());
    if let Some(reason) = vmm.unavailable_reason(Accel::Icount) {
        skipped.push((label, reason));
        return Ok(());
    }
    report::text(&format!("instruction counts on {label}"));
    let plain = Guest::Rhizo(build_kernel(Profile::Release, &["bench-cell"])?);
    let opts = RunOptions {
        stop_at_ready: false,
        timeout: RUN_TIMEOUT,
        echo: false,
        accel: Accel::Icount,
        gdb: false,
        network: false,
        pci_card: false,
        cpus: 1,
        memory_mib: vmm::DEFAULT_MEMORY_MIB,
        bench: None,
    };
    let mut samples = Samples::default();
    for guest in [&plain, profiled] {
        let out = vmm::run(vmm, guest, opts)?;
        if out.result == VmResult::Failed {
            skipped.push((label, format!("the run failed; output:\n{}", out.output)));
            return Ok(());
        }
        // A PMU that counts instructions is exact; else the clock is.
        let passes: Vec<_> = pmu_passes(&out.output)
            .into_iter()
            .filter(|(name, _)| name.ends_with("_insns"))
            .collect();
        let counts = if passes.is_empty() {
            counted(&out.output, "insns")
        } else {
            passes
        };
        for (name, value) in counts {
            samples.add(name, "insns", value);
        }
        for (name, value) in breakdowns(&out.output) {
            if !name.ends_with("_waits") {
                samples.add(format!("{name}_insns"), "insns", value);
            }
        }
    }
    metrics.extend(samples.metrics(&label));
    Ok(())
}

/// Samples of named figures over several runs, summarized as metrics.
#[derive(Debug, Default)]
pub struct Samples(BTreeMap<String, (&'static str, Vec<f64>)>);

impl Samples {
    /// Adds one run's value of `name`, in `unit`.
    pub fn add(&mut self, name: String, unit: &'static str, value: f64) {
        self.0
            .entry(name)
            .or_insert((unit, Vec::new()))
            .1
            .push(value);
    }

    /// One metric per name, for `vmm`.
    pub fn metrics(self, vmm: &str) -> Vec<Metric> {
        self.0
            .into_iter()
            .map(|(name, (unit, mut values))| {
                let (median, p90) = summarize(&mut values);
                Metric {
                    name,
                    guest: "rhizo".into(),
                    vmm: Some(vmm.into()),
                    unit: unit.into(),
                    median,
                    p90,
                    runs: values.len(),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: &str = "rhizo-ready cycles=1\n\
        rhizo-cell spawn_ns=100 run_ns=200 pt_frames=7 switch_ns=30 switch_untagged_ns=40\n\
        rhizo-ops syscall_ns=9\n\
        rhizo-pmu pmuv3 counters=6 instructions=yes\n\
        rhizo-pass instructions\n\
        rhizo-cell spawn_ns=1000 run_ns=2000 pt_frames=7 switch_ns=300 switch_untagged_ns=400\n\
        rhizo-threads create_ns=5 futex_wake_ns=6\n\
        rhizo-ops syscall_ns=90\n\
        rhizo-pass cycles\n\
        rhizo-cell spawn_ns=1500 run_ns=2500 pt_frames=7 switch_ns=350 switch_untagged_ns=450\n\
        rhizo-ops syscall_ns=95\n";

    fn find(v: &[(String, f64)], name: &str) -> Option<f64> {
        v.iter().find(|(n, _)| n == name).map(|(_, x)| *x)
    }

    #[test]
    fn counted_figures_of_one_pass() {
        let c = counted(RUN, "insns");
        assert_eq!(find(&c, "cell.spawn_insns"), Some(100.0), "first line");
        assert_eq!(find(&c, "cell.syscall_insns"), Some(9.0));
        assert_eq!(find(&c, "cell.fork_insns"), None, "not printed");
        assert!(c.iter().all(|(n, _)| !n.contains("pt_frames")));
    }

    #[test]
    fn pmu_passes_are_named_by_what_they_count() {
        let p = pmu_passes(RUN);
        assert_eq!(find(&p, "cell.spawn_insns"), Some(1000.0));
        assert_eq!(find(&p, "cell.syscall_insns"), Some(90.0));
        assert_eq!(find(&p, "cell.switch_cycles"), Some(350.0));
        assert_eq!(find(&p, "cell.syscall_cycles"), Some(95.0));
        assert!(
            p.iter().all(|(n, _)| !n.starts_with("cell.thread")),
            "threads time themselves"
        );
        assert!(pmu_passes("rhizo-cell spawn_ns=1\nrhizo-pmu none\n").is_empty());
        assert_eq!(pmu_line(RUN), Some("pmuv3 counters=6 instructions=yes"));
    }

    #[test]
    fn breakdown_and_lock_lines() {
        let out = "rhizo-ops cow_fault_ns=1100\n\
            rhizo-breakdown fork_touch total_ns=1000 user_ns=10 entry_exit_ns=5 cow_ns=700\n\
            rhizo-breakdown-counts fork_touch traps=3 cow=2\n\
            rhizo-breakdown-calls fork_touch Write=1/7\n\
            rhizo-locks frames=2/50 sched=0/0\n";
        let b = breakdowns(out);
        assert_eq!(find(&b, "breakdown.fork_touch.total"), Some(1000.0));
        assert_eq!(find(&b, "breakdown.fork_touch.cow"), Some(700.0));
        assert_eq!(find(&b, "lock.frames_waits"), Some(2.0));
        assert_eq!(find(&b, "lock.sched_waits"), Some(0.0));
        assert_eq!(b.len(), 6, "{b:?}");
    }

    #[test]
    fn samples_become_medians() {
        let mut s = Samples::default();
        for v in [3.0, 1.0, 2.0] {
            s.add("cell.x_insns".into(), "insns", v);
        }
        let m = s.metrics("qemu-microvm-icount");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].median, 2.0);
        assert_eq!(m[0].runs, 3);
        assert_eq!(m[0].key(), "cell.x_insns/rhizo/qemu-microvm-icount");
    }
}
