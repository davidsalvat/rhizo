//! The reference workloads (spec `benchmarks`, change
//! `add-reference-workloads`): a laptop running a few local agents next to
//! a model, and a server packed with active cells. A `bench-cell` kernel
//! booted with `bench=laptop` or `bench=server` runs one of them instead of
//! its cell figures and prints one line of results for `xtask`. The
//! in-kernel tests use [`fill_memory`].
#![cfg_attr(
    not(feature = "bench-cell"),
    allow(dead_code, reason = "the in-kernel tests use `fill_memory` only")
)]

use crate::arch::timer::{self, Counter};
use crate::cell::{Ending, Finished, Host, Resume, test_image};
use crate::memory;
use crate::prof::{self, Count};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use rhizo_cell::elf::ElfError;
use rhizo_core::options::Workload;
use rhizo_fs::Fs;
use rhizo_hal::{Console, ConsoleWriter, CycleCounter};
use rhizo_mm::FRAME_SIZE;

fn now_ns() -> u64 {
    timer::cycles_to_ns(Counter.cycles()).unwrap_or(0)
}

fn median(v: &mut [u64]) -> u64 {
    v.sort_unstable();
    v.get(v.len() / 2).copied().unwrap_or(0)
}

/// How many times `line` ends a line of `out`.
fn lines(out: &[u8], line: &[u8]) -> usize {
    out.split_inclusive(|&b| b == b'\n')
        .filter(|l| *l == line)
        .count()
}

/// Steps `host` until cell `id` has printed `line` `times` times; panics,
/// with what it printed, if it ends or takes more than 30 s.
fn wait_for(host: &mut Host, id: u32, line: &[u8], times: usize) {
    let end = now_ns() + 30_000_000_000;
    loop {
        if host.output(id).is_some_and(|o| lines(o, line) >= times) {
            return;
        }
        if let Some(f) = host.finished(id) {
            panic!(
                "cell {id} ended ({:?}) before printing {:?}: {}",
                f.ending,
                String::from_utf8_lossy(line),
                String::from_utf8_lossy(&f.output)
            );
        }
        let said = String::from_utf8_lossy(host.output(id).unwrap_or_default()).into_owned();
        assert!(now_ns() < end, "cell {id} is stuck; it printed: {said}");
        host.step();
    }
}

/// Runs the workload the command line asks for.
pub fn run(console: &mut impl Console, workload: Workload) {
    match workload {
        Workload::Laptop => laptop(console),
        Workload::Server => {
            server(console);
            let o = fill_memory();
            let _ = writeln!(
                ConsoleWriter(console),
                "rhizo-oom cells={} spawn_refused={} fork_refused={} resume_refused={} \
                 hog_enomem={} leaked_bytes={} heap_peak_bytes={} heap_kept_bytes={}",
                o.cells,
                o.spawn_refused as u8,
                o.fork_refused as u8,
                o.resume_refused as u8,
                o.hog_enomem as u8,
                o.leaked_bytes,
                o.heap_peak_bytes,
                o.heap_kept_bytes
            );
        }
    }
}

// --- Laptop (design D1) ----------------------------------------------------

/// Agents opened from the template, and how many stay live.
const AGENTS: usize = 50;
const LIVE: usize = 5;
/// Turns each live agent answers.
const TURNS: usize = 4;

/// The shared folder: no virtio-fs or vsock yet, so a file seeded into
/// the template's own files at boot, shared copy-on-write by every fork.
fn shared_folder() -> Fs {
    let mut notes = String::with_capacity(64 << 10);
    for i in 0.. {
        if notes.len() >= 64 << 10 {
            break;
        }
        let _ = writeln!(notes, "note {i:05}: agents read this shared folder");
    }
    let mut fs = Fs::new();
    fs.put_file(b"/shared/notes.txt", &notes.as_bytes()[..64 << 10])
        .expect("the shared notes fit");
    fs.mkdir(None, b"/work").expect("a work directory");
    fs
}

fn laptop(console: &mut impl Console) {
    let mut out = ConsoleWriter(console);
    let opts = crate::wire::options();
    let Some((ip, port)) = opts.llm else {
        let _ = writeln!(out, "rhizo-laptop skipped: no fake model server (llm=)");
        return;
    };
    let mut host = Host::new();
    if host.wire_address(10_000_000_000).is_none() {
        let _ = writeln!(out, "rhizo-laptop skipped: the network card got no address");
        return;
    }
    let idle_ms = opts.secs.unwrap_or(2) as u64 * 1000;
    let start = memory::free_bytes();
    let ip = format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
    let port = format!("{port}");
    let argv: [&[u8]; 3] = [b"agent", ip.as_bytes(), port.as_bytes()];
    let template = host
        .spawn_with_files(test_image("agent"), &argv, shared_folder(), true, false)
        .expect("spawn the agent");
    wait_for(&mut host, template, b"ready\n", 1);
    assert!(host.freeze(template));
    let after_template = memory::free_bytes();

    let ns = |t0: u64| timer::cycles_to_ns(Counter.cycles() - t0).unwrap_or(0);
    let (mut agents, mut open) = (Vec::new(), Vec::new());
    for _ in 0..AGENTS {
        let t0 = Counter.cycles();
        let id = host
            .spawn_from_template(template, true, false)
            .expect("open an agent");
        wait_for(&mut host, id, b"open\n", 1);
        open.push(ns(t0));
        agents.push(id);
    }
    let after_open = memory::free_bytes();
    let (mut snapshots, mut suspend) = (Vec::new(), Vec::new());
    for id in agents.split_off(LIVE) {
        let t0 = Counter.cycles();
        let snapshot = host.suspend(id).expect("suspend an agent");
        suspend.push(ns(t0));
        snapshots.push(snapshot);
    }
    let after_suspend = memory::free_bytes();
    let snapshot_bytes =
        snapshots.iter().map(|s| s.heap_bytes() as u64).sum::<u64>() / snapshots.len() as u64;

    // Idle: five agents wait for their users, 45 are suspended; one timer
    // cell ends the window, so the CPU idles as the scheduler makes it.
    let _ = writeln!(out, "rhizo-mark idle-start");
    let (w0, t0) = (prof::get(Count::IdleWakeups), Counter.cycles());
    let ms = format!("{idle_ms}");
    let timer = host
        .spawn(
            test_image("sleeper"),
            &[b"sleeper", ms.as_bytes()],
            true,
            false,
        )
        .expect("spawn the timer");
    host.run(|h| h.finished(timer).is_some());
    let idle_ns = ns(t0);
    let wakeups = prof::get(Count::IdleWakeups) - w0;
    let _ = writeln!(out, "rhizo-mark idle-end");

    let mut turn = Vec::new();
    for round in 1..=TURNS {
        for &id in &agents {
            let t0 = Counter.cycles();
            assert!(host.wake(id));
            wait_for(&mut host, id, b"turn\n", round);
            turn.push(ns(t0));
        }
    }
    let (mut resume, mut resume_turn) = (Vec::new(), Vec::new());
    for snapshot in snapshots {
        let t0 = Counter.cycles();
        let id = host
            .resume(snapshot, Resume::Lazy, true, false)
            .expect("resume an agent");
        resume.push(ns(t0));
        if resume_turn.len() < LIVE {
            let t0 = Counter.cycles();
            assert!(host.wake(id));
            wait_for(&mut host, id, b"turn\n", 1);
            resume_turn.push(ns(t0));
        }
        agents.push(id);
    }
    assert_eq!(host.live(), AGENTS, "every agent is still live");
    let _ = writeln!(
        out,
        "rhizo-laptop agents={AGENTS} suspended={} open_ns={} suspend_ns={} resume_ns={} \
         turn_ns={} resume_turn_ns={} memory_bytes={} template_bytes={} live_bytes={} \
         snapshot_bytes={snapshot_bytes} idle_ns={idle_ns} idle_wakeups={wakeups}",
        AGENTS - LIVE,
        median(&mut open),
        median(&mut suspend),
        median(&mut resume),
        median(&mut turn),
        median(&mut resume_turn),
        start - after_suspend,
        start - after_template,
        (after_template - after_open) / AGENTS as u64,
    );
}

// --- Server (design D2) ----------------------------------------------------

/// Every worker runs one operation per period.
const PERIOD_MS: u64 = 200;
/// The port the `serve` workers' shared listener takes.
const PORT: u16 = 7600;
/// Memory left free for what the workers take while they run.
const HEADROOM: u64 = 32 << 20;
/// The host suspends and resumes one worker, and forks one, this often.
const HOST_PERIOD_NS: u64 = 10_000_000;
/// One group of the mix: 4 computing, 2 on files, 2 pairs over TCP.
const MIX: [usize; 10] = [0, 0, 0, 0, 1, 1, 2, 3, 2, 3];
const MODES: [&[u8]; 4] = [b"cpu", b"file", b"serve", b"ping"];

/// The value after `key=` in `out`.
fn field(out: &[u8], key: &str) -> Option<u64> {
    let text = core::str::from_utf8(out).ok()?;
    text.split_whitespace()
        .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))?
        .parse()
        .ok()
}

fn server(console: &mut impl Console) {
    let opts = crate::wire::options();
    let target = opts.cells.unwrap_or(10_000) as usize;
    let secs = opts.secs.unwrap_or(3) as u64;
    let mut host = Host::new();
    let start = memory::free_bytes();
    let period = format!("{PERIOD_MS}");
    let (secs_arg, port) = (format!("{secs}"), format!("{PORT}"));
    let templates: Vec<u32> = MODES
        .iter()
        .map(|mode| {
            let argv: [&[u8]; 5] = [
                b"worker",
                mode,
                period.as_bytes(),
                secs_arg.as_bytes(),
                port.as_bytes(),
            ];
            let id = host
                .spawn(test_image("worker"), &argv, true, false)
                .expect("spawn a worker");
            wait_for(&mut host, id, b"ready\n", 1);
            assert!(host.freeze(id));
            id
        })
        .collect();

    // Fill: whole groups of the mix while memory keeps its headroom.
    let mut workers: Vec<(u32, usize)> = Vec::with_capacity(target);
    let mut limit = "none";
    prof::start(); // counts the cells made without a tag
    let fill = Counter.cycles();
    'fill: while workers.len() < target {
        if !memory::cells_may_take(HEADROOM) {
            limit = "memory";
            break;
        }
        for kind in MIX {
            match host.spawn_from_template(templates[kind], true, false) {
                Some(id) => workers.push((id, kind)),
                None => {
                    limit = "spawn-refused";
                    break 'fill;
                }
            }
        }
    }
    let fill_ns = timer::cycles_to_ns(Counter.cycles() - fill).unwrap_or(0);
    let untagged = prof::stop().count(Count::Untagged);
    let cells = workers.len();
    let mut peak = start - memory::free_bytes();

    // Load: the workers run for `secs`; meanwhile, at a constant rate, the
    // host suspends and resumes the next worker (its connections stay), and
    // forks the next one that computes or writes files (a fork's
    // connections are reset).
    prof::start();
    let t0 = now_ns();
    let stop_ops = t0 + secs * 1_000_000_000;
    let deadline = stop_ops + 5_000_000_000;
    let (mut next_op, mut turn, mut child) = (t0, 0, None);
    let (mut host_cycles, mut host_ops) = (0u64, 0u64);
    let (mut suspend_ns, mut resume_ns, mut fork_ns) = (Vec::new(), Vec::new(), Vec::new());
    loop {
        let now = now_ns();
        peak = peak.max(start - memory::free_bytes());
        if now >= next_op && now < stop_ops {
            next_op = now + HOST_PERIOD_NS;
            let c0 = Counter.cycles();
            if let Some(id) = child.take() {
                host.kill(id);
            }
            turn = (turn + 1) % workers.len();
            let (id, kind) = workers[turn];
            let c1 = Counter.cycles();
            if let Some(snapshot) = host.suspend(id) {
                let c2 = Counter.cycles();
                let new = host
                    .resume(snapshot, Resume::Lazy, true, false)
                    .expect("resume a worker");
                workers[turn] = (new, kind);
                suspend_ns.push(c2 - c1);
                resume_ns.push(Counter.cycles() - c2);
                host_ops += 2;
            }
            let parent = workers[turn - turn % MIX.len()].0; // computes
            let c3 = Counter.cycles();
            if let Some(ids) = host.fork(parent, 1) {
                fork_ns.push(Counter.cycles() - c3);
                child = ids.first().copied();
                host_ops += 1;
            }
            host_cycles += Counter.cycles() - c0;
        }
        if now >= stop_ops
            && let Some(id) = child.take()
        {
            host.kill(id);
        }
        if host.live() == child.is_some() as usize || now >= deadline {
            break;
        }
        let until = if now < stop_ops { next_op } else { deadline };
        host.run(|_| now_ns() >= until);
    }
    let elapsed = now_ns() - t0;
    // What each cell holds at the busiest point: its pages, tables, files,
    // sockets and kernel record.
    let per_cell = peak / cells.max(1) as u64;
    let report = prof::stop();
    let unfinished = workers.iter().filter(|&&(id, _)| host.kill(id)).count();

    let (mut ops, mut failed) = (0u64, Vec::new());
    for Finished {
        id, ending, output, ..
    } in host.take_finished()
    {
        match ending {
            Ending::Exit(0) => ops += field(&output, "ops").unwrap_or(0),
            Ending::Killed | Ending::Suspended => {}
            other => failed.push((id, other, output)),
        }
    }
    if let Some((id, ending, output)) = failed.first() {
        panic!(
            "{} workers failed; cell {id}: {ending:?}: {}",
            failed.len(),
            String::from_utf8_lossy(output)
        );
    }
    drop(host);

    let cyc = |c: u64| timer::cycles_to_ns(c).unwrap_or(0);
    let q = |h: &rhizo_core::stats::Histogram, p| h.quantile(p);
    let sys = &report.syscall_cycles;
    let mut out = ConsoleWriter(console);
    let _ = writeln!(
        out,
        "rhizo-server target={target} cells={cells} limit={limit} per_cell_bytes={per_cell} \
         fill_ns={fill_ns} ops={ops} elapsed_ns={elapsed} unfinished={unfinished} \
         sched_p50_ns={} sched_p99_ns={} sched_p999_ns={} wake_p50_ns={} wake_p99_ns={} \
         wake_p999_ns={} syscall_p50_ns={} syscall_p99_ns={} syscall_p999_ns={} io_woken={} \
         io_wasted={}",
        q(&report.sched, 500_000),
        q(&report.sched, 990_000),
        q(&report.sched, 999_000),
        q(&report.wake, 500_000),
        q(&report.wake, 990_000),
        q(&report.wake, 999_000),
        cyc(q(sys, 500_000)),
        cyc(q(sys, 990_000)),
        cyc(q(sys, 999_000)),
        report.count(Count::IoWoken),
        report.count(Count::IoWasted),
    );
    // Where the time went (design D5): idle, in system calls, in scheduling
    // decisions, in host operations; the rest is cell code, faults and
    // switches.
    let _ = writeln!(
        out,
        "rhizo-server-breakdown elapsed_ns={elapsed} idle_ns={} syscall_ns={} sched_ns={} \
         host_ns={} syscalls={} runs={} wakes={} io_woken={} net_changes={} cow_faults={} \
         lazy_faults={} interrupts={} untagged={untagged} host_ops={host_ops} suspend_ns={} resume_ns={} fork_ns={} \
         sched_max_ns={} wake_max_ns={}",
        cyc(report.count(Count::IdleCycles)),
        cyc(sys.sum()),
        cyc(report.count(Count::SchedCycles)),
        cyc(host_cycles),
        sys.count(),
        report.sched.count(),
        report.wake.count(),
        report.count(Count::IoWoken),
        report.count(Count::NetChanges),
        report.count(Count::CowFaults),
        report.count(Count::LazyFaults),
        report.count(Count::Interrupts),
        cyc(median(&mut suspend_ns)),
        cyc(median(&mut resume_ns)),
        cyc(median(&mut fork_ns)),
        report.sched.max(),
        report.wake.max(),
    );
}

// --- Out of memory (design D3) ---------------------------------------------

/// What happened when cells filled memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Oom {
    /// Idle cells forked from a template before the kernel refused one.
    pub cells: usize,
    /// Spawning from an executable then failed with `OutOfMemory`.
    pub spawn_refused: bool,
    /// Forking a live cell failed.
    pub fork_refused: bool,
    /// Resuming a snapshot eagerly failed.
    pub resume_refused: bool,
    /// With a quarter of the cells gone, `hog` got `ENOMEM` from `mmap`.
    pub hog_enomem: bool,
    /// Memory not back once every cell ended (0: none).
    pub leaked_bytes: u64,
    /// What the kernel heap grew by while memory was full.
    pub heap_peak_bytes: u64,
    /// Of what came back, what the kernel heap kept for itself: free blocks
    /// in pages it has not given back (change `add-heap-trim`).
    pub heap_kept_bytes: u64,
}

/// Memory not leaked: free frames plus the pages the kernel heap keeps, and
/// the heap's live allocations.
fn balance() -> (u64, usize) {
    let heap = crate::heap::stats();
    (
        memory::free_bytes() + heap.pages as u64 * FRAME_SIZE,
        heap.live,
    )
}

/// Forks idle cells (each run into its sleep) until the kernel refuses one,
/// then checks that every other way to take memory is refused too, never
/// with a panic, and that ending every cell gives all memory back.
pub fn fill_memory() -> Oom {
    let (before, live_before) = balance();
    let heap_before = crate::heap::stats().pages as u64 * FRAME_SIZE;
    let mut host = Host::new();
    let idle = test_image("idle");
    let template = host.spawn(idle, &[b"idle"], true, false).expect("spawn");
    wait_for(&mut host, template, b"idle\n", 1);
    let small = host.spawn(idle, &[b"idle"], true, false).expect("spawn");
    wait_for(&mut host, small, b"idle\n", 1);
    let snapshot = host.suspend(small).expect("suspend");
    assert!(host.freeze(template));
    let mut cells = Vec::new();
    while let Some(id) = host.spawn_from_template(template, false, false) {
        cells.push(id);
        host.step(); // into its sleep, taking the pages it writes
    }
    let spawn_refused = host.spawn(idle, &[b"idle"], false, false) == Err(ElfError::OutOfMemory);
    let fork_refused = host.fork(cells[0], 1).is_none();
    let resume_refused = host.resume(snapshot, Resume::Eager, false, false).is_err();
    let heap_peak = crate::heap::stats().pages as u64 * FRAME_SIZE;
    let count = cells.len();
    for id in cells.drain(..count / 4) {
        host.kill(id);
    }
    let hog = host
        .spawn(test_image("hog"), &[b"hog"], true, false)
        .expect("room for hog");
    host.run(|h| h.finished(hog).is_some());
    let hog_enomem = host
        .finished(hog)
        .is_some_and(|f| f.ending == Ending::Exit(0) && f.output == b"exhausted\n");
    drop(host);
    drop(cells);
    let (after, live_after) = balance();
    assert_eq!(
        live_after, live_before,
        "heap allocations left behind ({count} cells)"
    );
    Oom {
        cells: count,
        spawn_refused,
        fork_refused,
        resume_refused,
        hog_enomem,
        leaked_bytes: before.saturating_sub(after),
        heap_peak_bytes: heap_peak.saturating_sub(heap_before),
        heap_kept_bytes: (crate::heap::stats().pages as u64 * FRAME_SIZE)
            .saturating_sub(heap_before),
    }
}
