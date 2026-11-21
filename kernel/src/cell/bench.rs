//! `bench-cell`: the cell benchmarks the kernel prints (`cargo xtask
//! bench`), their clock, and the driver that runs them.

use super::*;

mod cells;
mod multi_core;
#[cfg(feature = "profile")]
mod profile;
mod workloads;

use cells::*;
use multi_core::*;
#[cfg(feature = "profile")]
use profile::*;
use workloads::*;

/// `bench-cell`: spawns and runs the hello cell repeatedly and prints the
/// median spawn time, the median time to run it to its exit (spawn
/// included), and its page-table frames.
#[cfg_attr(
    feature = "profile",
    allow(unreachable_code, reason = "a profile build runs other measures")
)]
pub fn bench(console: &mut impl Console) {
    use core::fmt::Write;
    #[cfg(feature = "profile")]
    return profiled(console);
    // The Node.js kernel measures Node.js only (its VM has the memory).
    if cfg!(feature = "node") {
        let _ = match node(false) {
            Some((cold, warm, _)) => writeln!(
                rhizo_hal::ConsoleWriter(console),
                "rhizo-node cold_ns={cold} warm_ns={warm}"
            ),
            None => writeln!(rhizo_hal::ConsoleWriter(console), "rhizo-node not packed"),
        };
        // With its JIT (change `node-jit`).
        if let Some((cold, warm, flips)) = node(true) {
            let _ = writeln!(
                rhizo_hal::ConsoleWriter(console),
                "rhizo-node-jit cold_ns={cold} warm_ns={warm} flips={flips}"
            );
        }
        return;
    }
    // The packages' kernel measures numpy only (its VM has the memory).
    if cfg!(feature = "packages") {
        let _ = match numpy() {
            Some((cold, warm)) => writeln!(
                rhizo_hal::ConsoleWriter(console),
                "rhizo-numpy cold_ns={cold} warm_ns={warm}"
            ),
            None => writeln!(rhizo_hal::ConsoleWriter(console), "rhizo-numpy not packed"),
        };
        return;
    }
    if let Some(workload) = crate::wire::options().bench {
        crate::workloads::run(console, workload);
        return;
    }
    if crate::smp::count() > 1 {
        let (serial, parallel, wake, calls_serial, calls_parallel) = multi_core();
        let (one, four, shootdown) = parallel_threads();
        let timer_error = timer_error();
        let (stall, stall_stopped) = host_op_stall();
        let _ = writeln!(
            rhizo_hal::ConsoleWriter(console),
            "rhizo-smp cpus={} serial_ns={serial} parallel_ns={parallel} ipi_wake_ns={wake} \
             threads1_ns={one} threads4_ns={four} shootdown_ns={shootdown} \
             timer_error_ns={timer_error} syscalls_serial_ns={calls_serial} \
             syscalls_parallel_ns={calls_parallel} host_op_stall_ns={stall} \
             host_op_stall_stopped_ns={stall_stopped}",
            crate::smp::count()
        );
        return; // the other metrics are measured with one CPU
    }
    benchmarks(console);
    // Again in retired instructions and in core cycles where the PMU counts
    // them (spec `profiling`): the same code measures, so the figures
    // compare.
    let mut w = rhizo_hal::ConsoleWriter(console);
    let Some(pmu) = crate::arch::pmu::init() else {
        let _ = writeln!(w, "rhizo-pmu none");
        return;
    };
    let _ = writeln!(
        w,
        "rhizo-pmu {} counters={} instructions={}",
        pmu.kind,
        pmu.counters,
        if pmu.instructions { "yes" } else { "no" }
    );
    for (clock, name) in [
        (CLOCK_INSTRUCTIONS, "instructions"),
        (CLOCK_CYCLES, "cycles"),
    ] {
        if clock == CLOCK_INSTRUCTIONS && !pmu.instructions {
            continue;
        }
        let _ = writeln!(rhizo_hal::ConsoleWriter(console), "rhizo-pass {name}");
        CLOCK.store(clock, Ordering::Relaxed);
        benchmarks(console);
    }
    CLOCK.store(CLOCK_TIME, Ordering::Relaxed);
}

/// What the benchmarks' clock counts (spec `profiling`).
static CLOCK: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(CLOCK_TIME);
const CLOCK_TIME: u8 = 0;
const CLOCK_INSTRUCTIONS: u8 = 1;
const CLOCK_CYCLES: u8 = 2;

/// The benchmarks' clock: the counter, or the PMU's retired instructions or
/// core cycles.
fn bench_clock() -> u64 {
    match CLOCK.load(Ordering::Relaxed) {
        CLOCK_TIME => Counter.cycles(),
        clock => crate::arch::pmu::read().map_or(0, |c| {
            if clock == CLOCK_INSTRUCTIONS {
                c.instructions.unwrap_or(0)
            } else {
                c.cycles
            }
        }),
    }
}

/// [`bench_clock`] ticks in the unit the benchmarks print: nanoseconds, or
/// instructions or cycles as counted.
fn bench_ns(ticks: u64) -> Option<u64> {
    match CLOCK.load(Ordering::Relaxed) {
        CLOCK_TIME => timer::cycles_to_ns(ticks),
        _ => Some(ticks),
    }
}

/// The benchmarks of one CPU, each printed as a `rhizo-…` line.
fn benchmarks(console: &mut impl Console) {
    use core::fmt::Write;
    let (spawn_ns, run_ns, tables) = hello_runs();
    let tagged = switch_ns(Host::new());
    let untagged = switch_ns(Host::new_untagged());
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-cell spawn_ns={spawn_ns} run_ns={run_ns} pt_frames={tables} switch_ns={tagged} \
         switch_untagged_ns={untagged}"
    );
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-fp switch_fp_ns={}",
        switch_fp_ns()
    );
    // KPTI's cost (design D5 of `add-kpti`): the same figures with it
    // forced on (`cell.syscall_ns` itself is on the `rhizo-ops` line).
    let default = user::kpti();
    user::set_kpti(true);
    let syscall_kpti = bench_ns(syscall_ticks()).unwrap_or(0);
    let switch_kpti = switch_ns(Host::new());
    let (_, run_kpti, _) = hello_runs();
    user::set_kpti(default);
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-kpti default={} syscall_kpti_ns={syscall_kpti} switch_kpti_ns={switch_kpti} \
         run_kpti_ns={run_kpti}",
        default as u8
    );
    // The first pass only: forking 50 MiB and writing it needs most of
    // the memory, which the kernel heap grown by the first pass keeps.
    if CLOCK.load(Ordering::Relaxed) == CLOCK_TIME {
        let f = fork_and_templates();
        let _ = writeln!(
            rhizo_hal::ConsoleWriter(console),
            "rhizo-fork fork_ns={} fork_touch_ns={} fork_pt_frames={} spawn_from_template_ns={}",
            f[0],
            f[1],
            f[2],
            f[3]
        );
    }
    let (cold, warm) = workload();
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-workload cold_ns={cold} warm_ns={warm}"
    );
    // Before the others on purpose: the interpreter churns through memory,
    // and the figures after it must not depend on that (design D2 of
    // `reuse-freed-frames`).
    if let Some((cold, warm)) = python() {
        let _ = writeln!(
            rhizo_hal::ConsoleWriter(console),
            "rhizo-python cold_ns={cold} warm_ns={warm}"
        );
    }
    if let Some((cold, warm)) = cpython() {
        let _ = writeln!(
            rhizo_hal::ConsoleWriter(console),
            "rhizo-cpython cold_ns={cold} warm_ns={warm}"
        );
    }
    match agent_figures() {
        Ok([turn, model, suspend_resume, fork]) => {
            let _ = writeln!(
                rhizo_hal::ConsoleWriter(console),
                "rhizo-agent turn_ns={turn} model_ns={model} suspend_resume_ns={suspend_resume} \
                 fork_ns={fork}"
            );
        }
        Err(why) => {
            let _ = writeln!(
                rhizo_hal::ConsoleWriter(console),
                "rhizo-agent skipped: {why}"
            );
        }
    }
    let (idle, suspended) = density();
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-density idle_bytes={idle} suspended_bytes={suspended}"
    );
    let (create, wake) = threads();
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-threads create_ns={create} futex_wake_ns={wake}"
    );
    let (write_read, fork_files) = files();
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-files write_read_ns={write_read} fork_ns={fork_files}"
    );
    let (pipe, loopback, tcp_resume) = net();
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-net pipe_rtt_ns={pipe} loopback_rtt_ns={loopback} tcp_resume_ns={tcp_resume}"
    );
    match wire_rtt() {
        Ok(rtt) => {
            let _ = writeln!(
                rhizo_hal::ConsoleWriter(console),
                "rhizo-wire tcp_rtt_ns={rtt}"
            );
        }
        Err(why) => {
            let _ = writeln!(
                rhizo_hal::ConsoleWriter(console),
                "rhizo-wire skipped: {why}"
            );
        }
    }
    let s = suspend_resume();
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-snapshot suspend_ns={} resume_ns={} resume_eager_ns={} resume_touch_ns={} bytes={} \
         export_ns={} import_ns={}",
        s[0],
        s[1],
        s[2],
        s[3],
        s[4],
        s[5],
        s[6]
    );
    let _ = writeln!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-ops syscall_ns={}",
        bench_ns(syscall_ticks()).unwrap_or(0)
    );
    // Cells sharing by weight (design D23 of `add-cell-hierarchy`): a
    // switch with a domain pick in every pass, the shares in time only.
    // After the other cell figures, whose heap it would leave otherwise.
    let _ = write!(
        rhizo_hal::ConsoleWriter(console),
        "rhizo-shares switch_ns={}",
        switch_domains_ns()
    );
    if CLOCK.load(Ordering::Relaxed) == CLOCK_TIME {
        let _ = write!(
            rhizo_hal::ConsoleWriter(console),
            " domain_share_pct={}",
            domain_share_pct()
        );
    }
    let _ = writeln!(rhizo_hal::ConsoleWriter(console));
    // Last: its files stay decoded in the kernel heap.
    if let Some((run, template)) = dynamic() {
        let _ = writeln!(
            rhizo_hal::ConsoleWriter(console),
            "rhizo-dynamic run_ns={run} template_ns={template}"
        );
    }
}

/// Rounds of a repeated measure (design D4 of `add-cell-files`).
const ROUNDS: usize = 5;

/// The minimum over [`ROUNDS`] consecutive rounds of each round's median:
/// the operation's cost, not the noise of its slowest moment.
fn robust(samples: &mut [u64]) -> u64 {
    let per = (samples.len() / ROUNDS).max(1);
    samples
        .chunks_mut(per)
        .take(ROUNDS)
        .map(|round| {
            round.sort_unstable();
            round[round.len() / 2]
        })
        .min()
        .unwrap_or(0)
}
