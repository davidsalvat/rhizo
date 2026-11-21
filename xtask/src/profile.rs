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
    ("rhizo-fp", "switch_fp_ns", "cell.switch_fp"),
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
    ("rhizo-snapshot", "export_ns", "cell.snapshot_export"),
    ("rhizo-snapshot", "import_ns", "cell.snapshot_import"),
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

/// The figures of a `bench-cell` run under `-icount`, in instructions: a
/// PMU pass that counts instructions is exact, so its figures win; the
/// clock (one nanosecond per instruction; AArch64's generic timer ticks
/// every 16) gives the rest, such as the fork benchmarks, which the passes
/// leave out.
pub fn icount_figures(output: &str) -> Vec<(String, f64)> {
    let mut figures: BTreeMap<String, f64> = counted(output, "insns").into_iter().collect();
    figures.extend(
        pmu_passes(output)
            .into_iter()
            .filter(|(name, _)| name.ends_with("_insns")),
    );
    figures.into_iter().collect()
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
    let profiled = profile_kernel()?;
    let one = RunOptions {
        stop_at_ready: false,
        timeout: RUN_TIMEOUT,
        echo: false,
        accel: opts.accel,
        gdb: false,
        network: false,
        pci_card: false,
        cpus: 1,
        memory_mib: crate::bench::CELL_BENCH_MEMORY_MIB,
        bench: None,
        shard: None,
        control: false,
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

/// Only the figures counted in instructions under QEMU with `-icount`
/// (`--icount-only`): what a host without KVM measures exactly, such as
/// GitHub's ARM64 runners (change `add-aarch64-bench`).
pub fn measure_icount(metrics: &mut Vec<Metric>, skipped: &mut Vec<(String, String)>) -> Result {
    icount(metrics, skipped, &profile_kernel()?)
}

/// Builds the `bench-cell` kernel with `profile`, kept apart: the next
/// kernel build writes the same path.
fn profile_kernel() -> Result<Guest> {
    let built = build_kernel(Profile::Release, &["bench-cell", "profile"])?;
    let mut name = built.file_stem().unwrap_or_default().to_os_string();
    name.push("-profile");
    let kept = built.with_file_name(name);
    let kept = match built.extension() {
        Some(ext) => kept.with_extension(ext),
        None => kept,
    };
    std::fs::copy(&built, &kept).map_err(|e| format!("cannot copy {}: {e}", built.display()))?;
    Ok(Guest::Rhizo(kept))
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
        memory_mib: crate::bench::CELL_BENCH_MEMORY_MIB,
        bench: None,
        shard: None,
        control: false,
    };
    let mut samples = Samples::default();
    for guest in [&plain, profiled] {
        let out = vmm::run(vmm, guest, opts)?;
        if out.result == VmResult::Failed {
            skipped.push((label, format!("the run failed; output:\n{}", out.output)));
            return Ok(());
        }
        for (name, value) in icount_figures(&out.output) {
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

// ---------------------------------------------------------------------------
// `cargo xtask profile`: samples folded per workload, cell and system call
// ---------------------------------------------------------------------------

/// Options of `cargo xtask profile`.
#[derive(Debug)]
pub struct ProfileArgs {
    pub vmm: Vmm,
    pub accel: Accel,
    pub out: std::path::PathBuf,
    pub per_cell: bool,
}

impl ProfileArgs {
    /// Parses `profile` arguments.
    pub fn parse(args: &[String]) -> Result<Self> {
        let mut o = Self {
            vmm: Vmm::default_for(crate::arch::current()),
            accel: Accel::Auto,
            out: crate::target_dir().join("profile"),
            per_cell: false,
        };
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let mut value = |name: &str| {
                it.next()
                    .cloned()
                    .ok_or_else(|| format!("{name} needs a value"))
            };
            match arg.as_str() {
                "--vmm" => o.vmm = Vmm::parse(&value("--vmm")?)?,
                "--accel" => o.accel = Accel::parse(&value("--accel")?)?,
                "--out" => o.out = value("--out")?.into(),
                "--per-cell" => o.per_cell = true,
                other => return Err(format!("unknown profile option `{other}`")),
            }
        }
        Ok(o)
    }
}

/// One workload of a sampling run: its samples and where its time went.
#[derive(Debug, Default, PartialEq)]
pub struct Profiled {
    pub workload: String,
    pub image: String,
    pub sampler: String,
    pub lost: u64,
    /// `(cell, address, count)`.
    pub samples: Vec<(u32, u64, u64)>,
    /// Parts of the breakdown in nanoseconds (`user`, `syscall`…).
    pub parts: Vec<(String, f64)>,
    /// System calls that took most time: `(name, calls, ns)`.
    pub calls: Vec<(String, u64, f64)>,
}

/// The workloads of a sampling run's output, in order.
pub fn profiled(output: &str) -> Vec<Profiled> {
    let mut out: Vec<Profiled> = Vec::new();
    let find = |out: &mut Vec<Profiled>, name: &str| -> usize {
        match out.iter().position(|p| p.workload == name) {
            Some(i) => i,
            None => {
                out.push(Profiled {
                    workload: name.into(),
                    ..Profiled::default()
                });
                out.len() - 1
            }
        }
    };
    for line in output.lines() {
        let mut words = line.split_whitespace();
        let (Some(kind), Some(workload)) = (words.next(), words.next()) else {
            continue;
        };
        let kv = |words: core::str::SplitWhitespace<'_>| -> Vec<(String, String)> {
            words
                .filter_map(|w| w.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        match kind {
            "rhizo-samples" => {
                let i = find(&mut out, workload);
                for (k, v) in kv(words) {
                    match k.as_str() {
                        "image" => out[i].image = v,
                        "sampler" => out[i].sampler = v,
                        "lost" => out[i].lost = v.parse().unwrap_or(0),
                        _ => {}
                    }
                }
            }
            "rhizo-sample" => {
                let rest: Vec<&str> = words.collect();
                if let [cell, pc, count] = rest[..]
                    && let (Ok(cell), Ok(pc), Ok(count)) =
                        (cell.parse(), u64::from_str_radix(pc, 16), count.parse())
                {
                    let i = find(&mut out, workload);
                    out[i].samples.push((cell, pc, count));
                }
            }
            "rhizo-breakdown" => {
                let i = find(&mut out, workload);
                out[i].parts = kv(words)
                    .into_iter()
                    .filter_map(|(k, v)| Some((k.strip_suffix("_ns")?.into(), v.parse().ok()?)))
                    .collect();
            }
            "rhizo-breakdown-calls" => {
                let i = find(&mut out, workload);
                out[i].calls = kv(words)
                    .into_iter()
                    .filter_map(|(name, v)| {
                        let (calls, ns) = v.split_once('/')?;
                        Some((name, calls.parse().ok()?, ns.parse().ok()?))
                    })
                    .collect();
            }
            _ => {}
        }
    }
    out
}

/// The function symbols of an ELF file, sorted by address, from `nm`.
pub fn symbols(elf: &std::path::Path) -> Vec<(u64, String)> {
    let out = std::process::Command::new("nm")
        .args(["-n", "-C", "--defined-only"])
        .arg(elf)
        .output();
    match out {
        Ok(o) if o.status.success() => parse_nm(&String::from_utf8_lossy(&o.stdout)),
        _ => Vec::new(),
    }
}

/// `nm -n` lines of code symbols (`T`, `t`, `W`, `w`) as `(address, name)`.
pub fn parse_nm(text: &str) -> Vec<(u64, String)> {
    let mut v: Vec<(u64, String)> = text
        .lines()
        .filter_map(|l| {
            let mut parts = l.splitn(3, ' ');
            let addr = u64::from_str_radix(parts.next()?, 16).ok()?;
            let kind = parts.next()?;
            let name = parts.next()?;
            matches!(kind, "T" | "t" | "W" | "w").then(|| (addr, name.to_string()))
        })
        .collect();
    v.sort_by_key(|s| s.0);
    v
}

/// The function at `pc`: the last symbol at or below it, else the 4 KiB
/// page of `image` it is in.
pub fn symbolize(symbols: &[(u64, String)], image: &str, pc: u64) -> String {
    match symbols.partition_point(|s| s.0 <= pc) {
        0 => format!("{image}+{:#x}", pc & !0xfff),
        i => symbols[i - 1].1.replace(';', ":"),
    }
}

/// Folded stacks of one workload (one `frames value` line each, the format
/// of `flamegraph.pl`, `inferno` and speedscope), in nanoseconds: the
/// samples share the cells' `user` time by count, under
/// `<workload>;<image>[;cell <id>];<function>`; the kernel's parts are
/// `<workload>;kernel;<part>`. Also the system calls alone:
/// `<workload>;<call>` (each call's own time, nested parts included).
pub fn folded(p: &Profiled, symbols: &[(u64, String)], per_cell: bool) -> (String, String) {
    let part = |name: &str| p.parts.iter().find(|x| x.0 == name).map_or(0.0, |x| x.1);
    let total: u64 = p.samples.iter().map(|s| s.2).sum();
    let per_sample = if total == 0 {
        0.0
    } else {
        part("user") / total as f64
    };
    let mut stacks: BTreeMap<String, f64> = BTreeMap::new();
    for &(cell, pc, count) in &p.samples {
        let function = symbolize(symbols, &p.image, pc);
        let stack = if per_cell {
            format!("{};{};cell {cell};{function}", p.workload, p.image)
        } else {
            format!("{};{};{function}", p.workload, p.image)
        };
        *stacks.entry(stack).or_default() += count as f64 * per_sample;
    }
    for (name, ns) in &p.parts {
        if !matches!(name.as_str(), "user" | "total") && *ns > 0.0 {
            stacks.insert(format!("{};kernel;{name}", p.workload), *ns);
        }
    }
    let mut out = String::new();
    for (stack, ns) in stacks {
        if ns.round() > 0.0 {
            out += &format!("{stack} {}\n", ns.round());
        }
    }
    let mut calls = String::new();
    for (name, _, ns) in &p.calls {
        calls += &format!("{};{name} {}\n", p.workload, ns.round());
    }
    (out, calls)
}

/// `cargo xtask profile`: runs the workloads with sampling on and writes
/// `<out>/<workload>.folded` and `<out>/<workload>.calls.folded`.
pub fn cmd(args: &ProfileArgs) -> Result {
    let kernel = build_kernel(Profile::Release, &["bench-cell", "sampling"])?;
    let cells = crate::build_cells()?;
    let opts = RunOptions {
        stop_at_ready: false,
        timeout: std::time::Duration::from_secs(300),
        echo: false,
        accel: args.accel,
        gdb: false,
        network: false,
        pci_card: false,
        cpus: 1,
        memory_mib: crate::bench::CELL_BENCH_MEMORY_MIB,
        bench: None,
        shard: None,
        control: false,
    };
    let run = vmm::run(args.vmm, &Guest::Rhizo(kernel), opts)?;
    if run.result == VmResult::Failed {
        return Err(format!("the profile run failed; output:\n{}", run.output));
    }
    std::fs::create_dir_all(&args.out)
        .map_err(|e| format!("cannot create {}: {e}", args.out.display()))?;
    let workloads = profiled(&run.output);
    if workloads.is_empty() {
        return Err(format!("no samples; output:\n{}", run.output));
    }
    for p in &workloads {
        let elf = match p.image.as_str() {
            "cpython" => crate::cpython::interpreter(),
            image => cells.join(image),
        };
        let symbols = symbols(&elf);
        let (stacks, calls) = folded(p, &symbols, args.per_cell);
        for (name, text) in [("folded", &stacks), ("calls.folded", &calls)] {
            let path = args.out.join(format!("{}.{name}", p.workload));
            std::fs::write(&path, text)
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        }
        summary(p, &stacks);
    }
    report::text(&format!(
        "folded stacks in {} (render with flamegraph.pl, inferno or speedscope)",
        args.out.display()
    ));
    Ok(())
}

/// Prints where a workload's time went: the heaviest functions and parts.
fn summary(p: &Profiled, stacks: &str) {
    let total = p
        .parts
        .iter()
        .find(|x| x.0 == "total")
        .map_or(0.0, |x| x.1)
        .max(1.0);
    let samples: u64 = p.samples.iter().map(|s| s.2).sum();
    report::text(&format!(
        "\n{}: {samples} samples ({} sampler, {} lost), {:.3} ms in all",
        p.workload,
        p.sampler,
        p.lost,
        total / 1e6
    ));
    let mut rows: Vec<(f64, &str)> = stacks
        .lines()
        .filter_map(|l| {
            let (stack, ns) = l.rsplit_once(' ')?;
            Some((ns.parse().ok()?, stack.split_once(';')?.1))
        })
        .collect();
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (ns, frame) in rows.iter().take(20) {
        report::text(&format!(
            "{:>7.2} %  {:>10.1} us  {frame}",
            100.0 * ns / total,
            ns / 1000.0
        ));
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

    const SAMPLED: &str = "rhizo-samples w image=cpython sampler=pmu period=100000 lost=2\n\
        rhizo-sample w 7 1010 3\n\
        rhizo-sample w 7 2000 1\n\
        rhizo-sample w 8 ffff0 1\n\
        rhizo-breakdown w total_ns=2000 user_ns=1000 entry_exit_ns=0 syscall_ns=600 copy_ns=400 lock_ns=0\n\
        rhizo-breakdown-counts w traps=3 syscall=2\n\
        rhizo-breakdown-calls w Mmap=2/500 Read=1/100\n";

    #[test]
    fn a_sampling_run_is_parsed_per_workload() {
        let p = profiled(SAMPLED);
        assert_eq!(p.len(), 1);
        let w = &p[0];
        assert_eq!(
            (w.image.as_str(), w.sampler.as_str(), w.lost),
            ("cpython", "pmu", 2)
        );
        assert_eq!(w.samples, [(7, 0x1010, 3), (7, 0x2000, 1), (8, 0xffff0, 1)]);
        assert!(w.parts.contains(&("user".into(), 1000.0)));
        assert_eq!(w.calls[0], ("Mmap".into(), 2, 500.0));
    }

    #[test]
    fn nm_output_gives_sorted_code_symbols() {
        let nm = "0000000000002000 T main\n0000000000001000 t helper\n\
                  0000000000003000 D data\n                 U undefined\n";
        let s = parse_nm(nm);
        assert_eq!(s, [(0x1000, "helper".into()), (0x2000, "main".into())]);
        assert_eq!(symbolize(&s, "img", 0x1fff), "helper");
        assert_eq!(symbolize(&s, "img", 0x2000), "main");
        assert_eq!(symbolize(&s, "img", 0x0fff), "img+0x0");
    }

    #[test]
    fn samples_share_user_time_and_parts_are_kernel_frames() {
        let p = &profiled(SAMPLED)[0];
        let symbols = parse_nm("0000000000001000 T PyEval\n0000000000002000 T memcpy\n");
        let (stacks, calls) = folded(p, &symbols, false);
        // 1000 ns of user time over 5 samples: 200 ns each.
        assert_eq!(
            stacks,
            "w;cpython;PyEval 600\nw;cpython;memcpy 400\n\
             w;kernel;copy 400\nw;kernel;syscall 600\n"
        );
        assert_eq!(calls, "w;Mmap 500\nw;Read 100\n");
        let (per_cell, _) = folded(p, &symbols, true);
        assert!(
            per_cell.contains("w;cpython;cell 7;PyEval 600\n"),
            "{per_cell}"
        );
        assert!(
            per_cell.contains("w;cpython;cell 8;memcpy 200\n"),
            "{per_cell}"
        );
    }

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
    fn icount_figures_take_the_pmu_pass_then_the_clock() {
        let run = format!("{RUN}rhizo-fork fork_ns=70\n");
        let f = icount_figures(&run);
        assert_eq!(find(&f, "cell.spawn_insns"), Some(1000.0), "PMU pass wins");
        assert_eq!(find(&f, "cell.syscall_insns"), Some(90.0));
        assert_eq!(find(&f, "cell.fork_insns"), Some(70.0), "clock only");
        assert!(f.iter().all(|(n, _)| n.ends_with("_insns")), "{f:?}");
        let no_pmu = icount_figures("rhizo-cell spawn_ns=5\nrhizo-pmu none\n");
        assert_eq!(no_pmu, [("cell.spawn_insns".to_string(), 5.0)]);
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
