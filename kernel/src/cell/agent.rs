//! The CPython agent run by in-kernel tests and benchmarks.

use super::*;

/// One run of the CPython agent (`cells/python/agent_loop.py`, change
/// `add-agent-workload`).
#[derive(Debug)]
pub struct AgentRun {
    /// Everything the agent printed: its turns, then each branch in order.
    pub transcript: Vec<u8>,
    /// Each suspend plus resume between turns.
    pub suspend_resume_ns: Vec<u64>,
    /// From freezing the agent at its decision point until every branch
    /// forked from it runs (has printed its first line).
    pub fork_ns: u64,
}

/// Branches the agent forks into at its decision point.
const AGENT_BRANCHES: u8 = 3;

/// How the agent reports ([`agent`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "each build uses its own: tests or benchmarks")]
pub enum AgentOutput {
    /// Its transcript only.
    Transcript,
    /// Its transcript and its turn times.
    Timed,
}

/// Runs the CPython agent (model `model`) for `turns` turns against the
/// host's model server (`llm=` on the command line), suspending and
/// resuming it in its pause after every turn (unless `mode` is `None`);
/// then, where it prints a line ending in `fork` and sleeps, freezes it as
/// a template and starts one cell per branch from it, each told its branch
/// in `/branch` (design D3 of `add-agent-workload`), and runs them to their
/// end. An error says why the agent could not run or finish.
pub fn agent(
    host: &mut Host,
    turns: usize,
    mode: Option<Resume>,
    model: &[u8],
    output: AgentOutput,
) -> Result<AgentRun, alloc::string::String> {
    use alloc::format;
    let files = cpython_files().ok_or("CPython not built")?;
    let (ip, port) = crate::wire::options()
        .llm
        .ok_or("no network card and model server on this VMM")?;
    host.wire_address(10_000_000_000)
        .ok_or("the network card got no address")?;
    let url = format!("http://{}.{}.{}.{}:{port}", ip[0], ip[1], ip[2], ip[3]);
    let turns_arg = format!("{turns}");
    let mut argv: Vec<&[u8]> = alloc::vec![
        b"/usr/bin/python3",
        b"/app/agent_loop.py",
        url.as_bytes(),
        model,
        b"cell",
        turns_arg.as_bytes(),
    ];
    match output {
        AgentOutput::Transcript => {}
        AgentOutput::Timed => argv.push(b"timed"),
    }
    let ns = |t0: u64, t1: u64| timer::cycles_to_ns(t1 - t0).unwrap_or(0);
    // Under emulation CPython is slow; a model that never answers must still
    // end the run.
    let limit_s = 600;
    let end = now_ns().saturating_add(limit_s * 1_000_000_000);
    let said = |host: &Host, id: u32| match host.finished(id) {
        Some(f) => format!(
            "it ended ({:?}) after: {}",
            f.ending,
            alloc::string::String::from_utf8_lossy(&f.output)
                .trim()
                .replace('\n', " | ")
        ),
        None => format!("not done in {limit_s} s"),
    };
    let mut id = host
        .spawn_with_files(test_image("cpython"), &argv, files, true, false)
        .map_err(|e| format!("cannot spawn CPython: {e:?}"))?;
    let mut run = AgentRun {
        transcript: Vec::new(),
        suspend_resume_ns: Vec::new(),
        fork_ns: 0,
    };
    // Its turns: after each, it prints the turn's line and pauses briefly,
    // and the host suspends and resumes it once it has read the line. The
    // host reads the output between slices only, and the pause can end
    // inside a slice (on a loaded host): the last turn's line then comes
    // with the decision point, so every line counts, not only the last
    // (design D1 of `fix-agent-turn-race`). At its decision point it waits,
    // asleep, for `/branch`.
    let mut watch = rhizo_core::agent::Watch::default();
    let (mut read, mut turns, mut forked) = (0, 0, false);
    loop {
        let seen = host.output(id).map_or(Default::default(), |o| {
            let seen = watch.read(&o[read..]);
            read = o.len();
            seen
        });
        forked |= seen.fork;
        turns += seen.turns;
        if let Some(mode) = mode
            && seen.turns > 0
        {
            if seen.turns > 1 {
                return Err(format!(
                    "turns {} to {turns} ended before the host looked",
                    turns + 1 - seen.turns
                ));
            }
            let t0 = Counter.cycles();
            let snapshot = host.suspend(id).ok_or("cannot suspend the agent")?;
            let resumed = host.resume(snapshot, mode, true, false);
            let t2 = Counter.cycles();
            run.suspend_resume_ns.push(ns(t0, t2));
            let resumed = resumed.map_err(|_| "cannot resume the agent")?;
            run.transcript
                .extend_from_slice(&host.finished(id).expect("suspended").output);
            id = resumed;
            read = 0;
            continue;
        }
        if forked && host.asleep(id) {
            break;
        }
        if host.finished(id).is_some() || now_ns() > end {
            return Err(said(host, id));
        }
        host.step();
    }
    run.transcript
        .extend_from_slice(host.output(id).unwrap_or_default());
    // The decision point: one cell per branch, from the frozen agent.
    let t0 = Counter.cycles();
    if !host.freeze(id) {
        return Err("cannot freeze the agent".into());
    }
    let mut branches = Vec::new();
    for b in 0..AGENT_BRANCHES {
        let child = host
            .spawn_from_template(id, true, false)
            .ok_or("cannot fork the agent")?;
        if !host.write_file(child, b"/branch", &[b'0' + b]) {
            return Err("cannot give a branch its number".into());
        }
        branches.push(child);
    }
    let running = |host: &mut Host, child: u32| {
        let out = match host.finished(child) {
            Some(f) => Some(f.output.as_slice()),
            None => host.output(child),
        };
        out.is_some_and(|o| o.contains(&b'\n'))
    };
    while !branches.iter().all(|&c| running(host, c)) {
        if now_ns() > end {
            return Err("the branches do not run".into());
        }
        host.step();
    }
    run.fork_ns = ns(t0, Counter.cycles());
    host.run_until(end, |h| branches.iter().all(|&c| h.finished(c).is_some()));
    for &child in &branches {
        match host.finished(child) {
            Some(f) if f.ending == Ending::Exit(0) => {
                run.transcript.extend_from_slice(&f.output);
            }
            _ => {
                let why = said(host, child);
                host.kill(child);
                return Err(format!("branch {child}: {why}"));
            }
        }
    }
    Ok(run)
}
