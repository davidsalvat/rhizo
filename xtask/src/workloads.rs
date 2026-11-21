//! The reference workloads of `cargo xtask bench` (change
//! `add-reference-workloads`): a `bench-cell` kernel booted with
//! `bench=laptop` or `bench=server` per run, its result line turned into
//! metrics, and the host's view added (the VMM's CPU time while the guest
//! idles).

use crate::bench::{Metric, Only, Options, RUN_TIMEOUT, deterministic, summarize};
use crate::vmm::{self, Bench, Guest, Mark, RunOptions, VmResult, VmRun, Vmm};
use crate::{Profile, Result, build_kernel, report};
use std::collections::BTreeMap;
use std::time::Duration;

/// Memory of the laptop VM: the agents, their snapshots and the template.
const LAPTOP_MEMORY_MIB: u32 = 512;
/// Virtual CPUs of the laptop VM: a laptop gives its agents more than one,
/// and every CPU idles while the agents wait.
const LAPTOP_CPUS: u32 = 2;
/// Memory of the server VM: 10,000 active cells and the out-of-memory push.
const SERVER_MEMORY_MIB: u32 = 1024;

/// Measures the workloads `opts` asks for on every VMM available.
pub fn measure(
    opts: &Options,
    metrics: &mut Vec<Metric>,
    skipped: &mut Vec<(String, String)>,
) -> Result {
    let kernel = build_kernel(Profile::Release, &["bench-cell"])?;
    let guest = Guest::Rhizo(kernel);
    for &vmm in &opts.vmms {
        if let Some(why) = vmm.unavailable_reason(opts.accel) {
            if opts.workload.is_some() {
                skipped.push((vmm.id().into(), why));
            }
            continue; // otherwise already reported by the boot benchmark
        }
        if opts.workload != Some(Only::Server) {
            laptop(opts, vmm, &guest, metrics, skipped)?;
        }
        if opts.workload != Some(Only::Laptop) {
            server(opts, vmm, &guest, metrics)?;
        }
    }
    Ok(())
}

/// Adds one metric of `vmm` from its samples (none: nothing).
fn push(metrics: &mut Vec<Metric>, vmm: Vmm, name: &str, unit: &str, values: &mut [f64]) {
    if values.is_empty() {
        return;
    }
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

/// The `key=value` numbers of the line of `output` starting with `prefix`.
fn fields(output: &str, prefix: &str) -> Option<BTreeMap<String, u64>> {
    let line = output.lines().find(|l| l.starts_with(prefix))?;
    Some(
        line.split_whitespace()
            .filter_map(|kv| {
                let (k, v) = kv.split_once('=')?;
                Some((k.to_string(), v.parse().ok()?))
            })
            .collect(),
    )
}

/// The VMM's share of one host CPU between the marks `from` and `to`, in
/// percent (design D6).
fn host_cpu_pct(marks: &[Mark], from: &str, to: &str) -> Option<f64> {
    let find = |name: &str| marks.iter().find(|m| m.name == name);
    let (a, b) = (find(from)?, find(to)?);
    let wall = b.at.checked_sub(a.at)?.as_nanos() as f64;
    let cpu = b.cpu_ns?.checked_sub(a.cpu_ns?)? as f64;
    (wall > 0.0).then(|| 100.0 * cpu / wall)
}

/// A run that went wrong: what to tell the developer.
fn failed(what: &str, vmm: Vmm, out: &VmRun) -> String {
    let tail: String = out
        .output
        .lines()
        .rev()
        .take(30)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "the {what} workload failed on {} ({:?}); last output:\n{tail}",
        vmm.id(),
        out.result
    )
}

/// The figures of one laptop run (design D1).
#[derive(Debug, PartialEq)]
struct Laptop {
    values: Vec<(&'static str, &'static str, f64)>,
}

/// Reads a laptop run; the reason when the guest skipped it.
fn parse_laptop(out: &VmRun) -> std::result::Result<Laptop, String> {
    if let Some(line) = out
        .output
        .lines()
        .find(|l| l.starts_with("rhizo-laptop skipped: "))
    {
        return Err(line["rhizo-laptop skipped: ".len()..].trim().to_string());
    }
    let f = fields(&out.output, "rhizo-laptop ").ok_or("no `rhizo-laptop` line")?;
    let get = |k: &str| {
        f.get(k)
            .copied()
            .ok_or(format!("no `{k}` in the laptop line"))
    };
    let us = |k: &str| get(k).map(|v| v as f64 / 1000.0);
    let idle_s = get("idle_ns")? as f64 / 1e9;
    let mut values = vec![
        ("laptop.open_us", "us", us("open_ns")?),
        ("laptop.suspend_us", "us", us("suspend_ns")?),
        ("laptop.resume_us", "us", us("resume_ns")?),
        ("workload.agent_turn_us", "us", us("turn_ns")?),
        ("laptop.resume_turn_us", "us", us("resume_turn_ns")?),
        ("laptop.memory_bytes", "bytes", get("memory_bytes")? as f64),
        (
            "laptop.idle_wakeups_per_s",
            "/s",
            get("idle_wakeups")? as f64 / idle_s,
        ),
        (
            "laptop.turn_wakeups",
            "wakeups",
            get("turn_wakeups")? as f64,
        ),
    ];
    if let Some(pct) = host_cpu_pct(&out.marks, "idle-start", "idle-end") {
        values.push(("laptop.idle_host_cpu_pct", "%", pct));
    }
    Ok(Laptop { values })
}

fn laptop(
    opts: &Options,
    vmm: Vmm,
    guest: &Guest,
    metrics: &mut Vec<Metric>,
    skipped: &mut Vec<(String, String)>,
) -> Result {
    let what = format!("laptop/{}", vmm.id());
    if let crate::hostnet::Network::None(why) = crate::hostnet::for_vmm(vmm) {
        skipped.push((what, why));
        return Ok(());
    }
    report::text(&format!("laptop workload on {}", vmm.id()));
    let idle_secs = if opts.soak { 60 } else { 2 };
    let run = RunOptions {
        stop_at_ready: false,
        timeout: RUN_TIMEOUT + Duration::from_secs(idle_secs.into()),
        echo: false,
        accel: opts.accel,
        gdb: false,
        network: true,
        pci_card: false,
        cpus: LAPTOP_CPUS,
        memory_mib: LAPTOP_MEMORY_MIB,
        bench: Some(Bench::Laptop {
            idle_secs,
            llm_delay_ms: opts.llm_delay_ms,
        }),
        shard: None,
        control: false,
    };
    let mut samples: BTreeMap<(&str, &str), Vec<f64>> = BTreeMap::new();
    for _ in 0..opts.runs.clamp(1, 3) {
        let out = vmm::run(vmm, guest, run)?;
        if out.result == VmResult::Failed {
            return Err(failed("laptop", vmm, &out));
        }
        match parse_laptop(&out) {
            Ok(l) => {
                if let Some(line) = out.output.lines().find(|l| l.starts_with("rhizo-laptop ")) {
                    report::text(line);
                }
                for (name, unit, v) in l.values {
                    samples.entry((name, unit)).or_default().push(v);
                }
            }
            Err(why) if out.output.contains("rhizo-laptop skipped: ") => {
                skipped.push((what, why));
                return Ok(());
            }
            Err(why) => return Err(format!("{why}\n{}", failed("laptop", vmm, &out))),
        }
    }
    for ((name, unit), mut values) in samples {
        push(metrics, vmm, name, unit, &mut values);
    }
    Ok(())
}

/// The figures of one server run (design D2) and its out-of-memory checks
/// (design D3).
fn parse_server(
    out: &VmRun,
) -> std::result::Result<Vec<(&'static str, &'static str, f64)>, String> {
    let f = fields(&out.output, "rhizo-server ").ok_or("no `rhizo-server` line")?;
    let get = |k: &str| {
        f.get(k)
            .copied()
            .ok_or(format!("no `{k}` in the server line"))
    };
    let oom = fields(&out.output, "rhizo-oom ").ok_or("no `rhizo-oom` line")?;
    let clean = [
        "spawn_refused",
        "fork_refused",
        "resume_refused",
        "hog_enomem",
    ]
    .iter()
    .all(|k| oom.get(*k) == Some(&1))
        && oom.get("leaked_bytes") == Some(&0);
    if !clean {
        return Err(format!(
            "memory running out was not refused cleanly: {:?}",
            out.output.lines().find(|l| l.starts_with("rhizo-oom "))
        ));
    }
    let us = |k: &str| get(k).map(|v| v as f64 / 1000.0);
    let elapsed_s = get("elapsed_ns")? as f64 / 1e9;
    let woken = get("io_woken")?;
    let wasted = 100.0 * get("io_wasted")? as f64 / woken.max(1) as f64;
    Ok(vec![
        ("density.cells", "cells", get("cells")? as f64),
        ("density.cell_bytes", "bytes", get("per_cell_bytes")? as f64),
        ("density.ops_per_s", "ops/s", get("ops")? as f64 / elapsed_s),
        ("density.sched_p50_us", "us", us("sched_p50_ns")?),
        ("density.sched_p99_us", "us", us("sched_p99_ns")?),
        ("density.sched_p999_us", "us", us("sched_p999_ns")?),
        ("density.wake_p50_us", "us", us("wake_p50_ns")?),
        ("density.wake_p99_us", "us", us("wake_p99_ns")?),
        ("density.wake_p999_us", "us", us("wake_p999_ns")?),
        (
            "density.syscall_p50_ns",
            "ns",
            get("syscall_p50_ns")? as f64,
        ),
        (
            "density.syscall_p99_ns",
            "ns",
            get("syscall_p99_ns")? as f64,
        ),
        (
            "density.syscall_p999_ns",
            "ns",
            get("syscall_p999_ns")? as f64,
        ),
        ("density.wasted_wakeups_pct", "%", wasted),
        ("density.oom_cells", "cells", oom["cells"] as f64),
    ])
}

fn server(opts: &Options, vmm: Vmm, guest: &Guest, metrics: &mut Vec<Metric>) -> Result {
    report::text(&format!("server workload on {}", vmm.id()));
    let secs: u16 = if opts.soak { 600 } else { 3 };
    let run = RunOptions {
        stop_at_ready: false,
        timeout: RUN_TIMEOUT + Duration::from_secs(secs.into()),
        echo: false,
        accel: opts.accel,
        gdb: false,
        network: false,
        pci_card: false,
        cpus: 1,
        memory_mib: SERVER_MEMORY_MIB,
        bench: Some(Bench::Server {
            cells: opts.cells,
            secs,
        }),
        shard: None,
        control: false,
    };
    let runs = if opts.soak { 1 } else { opts.runs.clamp(1, 3) };
    let mut samples: BTreeMap<(&str, &str), Vec<f64>> = BTreeMap::new();
    for _ in 0..runs {
        let out = vmm::run(vmm, guest, run)?;
        if out.result == VmResult::Failed {
            return Err(failed("server", vmm, &out));
        }
        let values =
            parse_server(&out).map_err(|why| format!("{why}\n{}", failed("server", vmm, &out)))?;
        for line in out
            .output
            .lines()
            .filter(|l| l.starts_with("rhizo-server") || l.starts_with("rhizo-oom"))
        {
            report::text(line);
        }
        for (name, unit, v) in values {
            samples.entry((name, unit)).or_default().push(v);
        }
    }
    for ((name, unit), mut values) in samples {
        if name == "density.cells" {
            // The same at every run: memory and the target decide it.
            metrics.push(Metric {
                vmm: Some(vmm.id().into()),
                ..deterministic(name, unit, values[0])
            });
        } else {
            push(metrics, vmm, name, unit, &mut values);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(output: &str, marks: Vec<Mark>) -> VmRun {
        VmRun {
            result: VmResult::Passed,
            output: output.into(),
            time_to_ready: None,
            ready_cycles: None,
            marks,
        }
    }

    fn mark(name: &str, ms: u64, cpu_ms: u64) -> Mark {
        Mark {
            name: name.into(),
            at: Duration::from_millis(ms),
            cpu_ns: Some(cpu_ms * 1_000_000),
        }
    }

    #[test]
    fn laptop_line_and_marks_give_the_metrics() {
        let out = run(
            "rhizo-mark idle-start\nrhizo-mark idle-end\n\
             rhizo-laptop agents=50 suspended=45 open_ns=30000 suspend_ns=900000 \
             resume_ns=2000 turn_ns=150000 resume_turn_ns=400000 memory_bytes=60000000 \
             template_bytes=1200000 live_bytes=40000 snapshot_bytes=1100000 \
             idle_ns=2000000000 idle_wakeups=4 turn_wakeups=3\n",
            vec![mark("idle-start", 1000, 50), mark("idle-end", 3000, 60)],
        );
        let l = parse_laptop(&out).unwrap();
        let get = |name: &str| l.values.iter().find(|v| v.0 == name).map(|v| v.2);
        assert_eq!(get("laptop.open_us"), Some(30.0));
        assert_eq!(get("workload.agent_turn_us"), Some(150.0));
        assert_eq!(get("laptop.resume_us"), Some(2.0));
        assert_eq!(get("laptop.memory_bytes"), Some(60_000_000.0));
        assert_eq!(get("laptop.idle_wakeups_per_s"), Some(2.0));
        assert_eq!(get("laptop.turn_wakeups"), Some(3.0));
        assert_eq!(get("laptop.idle_host_cpu_pct"), Some(0.5));
    }

    #[test]
    fn a_skipped_laptop_run_says_why() {
        let out = run(
            "rhizo-laptop skipped: no fake model server (llm=)\n",
            vec![],
        );
        assert_eq!(
            parse_laptop(&out),
            Err("no fake model server (llm=)".to_string())
        );
    }

    #[test]
    fn host_cpu_needs_both_marks_and_proc() {
        let marks = vec![mark("idle-start", 0, 0), mark("idle-end", 2000, 20)];
        assert_eq!(host_cpu_pct(&marks, "idle-start", "idle-end"), Some(1.0));
        assert_eq!(host_cpu_pct(&marks[..1], "idle-start", "idle-end"), None);
        let mut no_proc = marks;
        no_proc[1].cpu_ns = None;
        assert_eq!(host_cpu_pct(&no_proc, "idle-start", "idle-end"), None);
    }

    const SERVER: &str = "rhizo-server target=10000 cells=10000 limit=none \
        per_cell_bytes=40000 fill_ns=1 ops=150000 elapsed_ns=3000000000 unfinished=0 \
        sched_p50_ns=2000 sched_p99_ns=90000 sched_p999_ns=400000 wake_p50_ns=3000 \
        wake_p99_ns=120000 wake_p999_ns=900000 syscall_p50_ns=300 syscall_p99_ns=5000 \
        syscall_p999_ns=20000 io_woken=4000 io_wasted=1000\n";

    #[test]
    fn server_line_gives_the_density_metrics() {
        let out = run(
            &format!(
                "{SERVER}rhizo-oom cells=25000 spawn_refused=1 fork_refused=1 \
                 resume_refused=1 hog_enomem=1 leaked_bytes=0\n"
            ),
            vec![],
        );
        let values = parse_server(&out).unwrap();
        let get = |name: &str| values.iter().find(|v| v.0 == name).map(|v| v.2);
        assert_eq!(get("density.cells"), Some(10000.0));
        assert_eq!(get("density.ops_per_s"), Some(50000.0));
        assert_eq!(get("density.wake_p99_us"), Some(120.0));
        assert_eq!(get("density.sched_p999_us"), Some(400.0));
        assert_eq!(get("density.syscall_p99_ns"), Some(5000.0));
        assert_eq!(get("density.oom_cells"), Some(25000.0));
        assert_eq!(get("density.wasted_wakeups_pct"), Some(25.0));
    }

    #[test]
    fn unclean_out_of_memory_fails_the_run() {
        for oom in [
            "rhizo-oom cells=9 spawn_refused=0 fork_refused=1 resume_refused=1 hog_enomem=1 \
             leaked_bytes=0",
            "rhizo-oom cells=9 spawn_refused=1 fork_refused=1 resume_refused=1 hog_enomem=1 \
             leaked_bytes=4096",
            "",
        ] {
            let out = run(&format!("{SERVER}{oom}\n"), vec![]);
            assert!(parse_server(&out).is_err(), "{oom}");
        }
    }
}
