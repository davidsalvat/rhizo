//! `rhizo run python-agent`: the CPython agent of `add-agent-workload`
//! against an OpenAI-compatible model, driven through the control plane
//! (design D6 of `add-control-plane`). The host decides everything: it
//! suspends and resumes the agent after every turn, and at the agent's
//! decision point forks it into branches, tells each its number in
//! `/branch` and ends the agent itself; the kernel only executes.

use crate::args;
use crate::client::{Client, Reply, duration};
use rhizo_control::{Body, Message, Request, Run, State};
use std::collections::HashMap;
use std::time::Instant;
use xtask::vmm::Vmm;

/// Branches the agent forks into.
const BRANCHES: u32 = 3;

/// `IP:PORT` of the model as a guest on `vmm` reaches it: the host's own
/// address there for a model on the host's loopback (or listening on every
/// address), any other IPv4 address as it is.
pub fn guest_endpoint(url: &str, vmm: Vmm) -> Result<String, String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let hostport = rest.split('/').next().unwrap_or_default();
    let (host, port) = hostport
        .rsplit_once(':')
        .ok_or_else(|| format!("`{url}` names no port (http://HOST:PORT)"))?;
    let port: u16 = port
        .parse()
        .map_err(|_| format!("`{port}` is not a port"))?;
    let host = match host {
        "localhost" | "127.0.0.1" | "0.0.0.0" => xtask::hostnet::host_address(vmm),
        ip if ip.parse::<std::net::Ipv4Addr>().is_ok() => ip,
        other => return Err(format!("`{other}`: give the model's IPv4 address")),
    };
    Ok(format!("{host}:{port}"))
}

fn say(text: &str) {
    println!("[rhizo] {text}");
}

fn cells(reply: &Reply) -> Result<Vec<u32>, String> {
    match &reply.body {
        Body::Cells(ids) => Ok(ids.clone()),
        other => Err(format!("unexpected reply {other:?}")),
    }
}

/// Whether the agent's `line` ends a turn (`live`: its readable account).
fn ends_turn(line: &str, live: bool) -> bool {
    if live {
        line.starts_with("[turn ") && (line.contains("] tool:") || line.contains("] no tool call"))
    } else {
        line.starts_with("turn ")
    }
}

fn asks_for_fork(line: &str, live: bool) -> bool {
    if live {
        line.contains("decision point")
    } else {
        line == "fork"
    }
}

/// Runs the agent to the end of its branches.
pub fn run(client: &mut Client, run: &args::Run) -> Result<(), String> {
    let started = Instant::now();
    let vmm = Vmm::parse(&client.info.vmm)?;
    let model = run.model.clone().unwrap_or_default();
    let url = format!(
        "http://{}",
        guest_endpoint(run.llm.as_deref().unwrap_or_default(), vmm)?
    );
    let live = !run.transcript;
    let mut argv: Vec<Vec<u8>> = [
        "/usr/bin/python3",
        "/app/agent_loop.py",
        &url,
        &model,
        "cell",
        &run.turns.to_string(),
    ]
    .iter()
    .map(|a| a.as_bytes().to_vec())
    .collect();
    if live {
        argv.push(b"live".to_vec());
    }
    let reply = client.call(&Request::Run(Run {
        image: b"cpython".to_vec(),
        files_from: b"cpython-files".to_vec(),
        argv,
        ..Run::default()
    }))?;
    let &Body::Cell(agent) = &reply.body else {
        return Err(format!("unexpected reply {:?}", reply.body));
    };
    say(&format!(
        "cell {agent}: CPython with `agent_loop.py`, model `{model}` at {url} as the guest sees \
         it; started in {}",
        reply.timing()
    ));
    client.send(&Request::Output(agent))?;
    let mut partial: HashMap<u32, Vec<u8>> = HashMap::new();
    let mut suspends = Vec::new();
    let mut fork_ns = 0;
    let mut branches: Vec<u32> = Vec::new();
    let mut transcripts: HashMap<u32, String> = HashMap::new();
    let mut running = 0;
    let mut fork_started = None;
    let mut ended = 0;
    loop {
        let (_, message) = client.next()?;
        let (cell, data) = match message {
            Message::Output { cell, data } => (cell, data),
            Message::Ended { cell, state } if cell == agent => {
                if branches.is_empty() {
                    return Err(format!("the agent ended before its fork: {state:?}"));
                }
                continue;
            }
            Message::Ended { cell, state } => {
                if state != State::Exited(0) {
                    return Err(format!("branch cell {cell} ended: {state:?}"));
                }
                ended += 1;
                if ended == branches.len() {
                    break;
                }
                continue;
            }
            Message::Failed { error, message, .. } => {
                return Err(format!(
                    "{}: {}",
                    error.as_str(),
                    String::from_utf8_lossy(&message)
                ));
            }
            _ => continue,
        };
        let buf = partial.entry(cell).or_default();
        buf.extend_from_slice(&data);
        let mut lines = Vec::new();
        while let Some(end) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=end).collect();
            lines.push(String::from_utf8_lossy(&line[..end]).into_owned());
        }
        for line in lines {
            if cell != agent {
                let text = transcripts.entry(cell).or_default();
                if text.is_empty() {
                    running += 1;
                    if running == branches.len()
                        && let Some(t0) = fork_started
                    {
                        say(&format!(
                            "all {BRANCHES} branches running {} after the fork request",
                            duration(Instant::now().duration_since(t0).as_nanos() as u64)
                        ));
                    }
                }
                if live {
                    println!("{line}");
                }
                text.push_str(&line);
                text.push('\n');
                continue;
            }
            println!("{line}");
            if ends_turn(&line, live) && suspends.len() < run.turns as usize {
                let s = client.call(&Request::Suspend(agent))?;
                let &Body::Bytes(bytes) = &s.body else {
                    return Err(format!("unexpected reply {:?}", s.body));
                };
                let r = client.call(&Request::Resume(agent))?;
                say(&format!(
                    "suspended between turns ({} KiB snapshot, connection parked) in {}, \
                     resumed in {}",
                    bytes / 1024,
                    s.timing(),
                    r.timing()
                ));
                suspends.push(s.ns + r.ns);
            } else if asks_for_fork(&line, live) && branches.is_empty() {
                fork_started = Some(Instant::now());
                let f = client.call(&Request::Fork {
                    cell: agent,
                    n: BRANCHES,
                })?;
                fork_ns = f.ns;
                branches = cells(&f)?;
                for (b, &child) in branches.iter().enumerate() {
                    client.call(&Request::WriteFile {
                        cell: child,
                        path: b"/branch".to_vec(),
                        data: b.to_string().into_bytes(),
                    })?;
                    client.send(&Request::Output(child))?;
                }
                // The agent itself waits at its decision point for good.
                client.call(&Request::Kill(agent))?;
                say(&format!(
                    "forked the agent at its decision point into {BRANCHES} branches \
                     (cells {branches:?}, copy-on-write, own connections) in {}",
                    f.timing()
                ));
            }
        }
    }
    if !live {
        for b in &branches {
            print!("{}", transcripts.get(b).map_or("", String::as_str));
        }
    }
    suspends.sort_unstable();
    println!(
        "rhizo-demo done in {} ms: {} suspend and resume (median {}), fork into {BRANCHES} \
         branches {} (kernel times)",
        started.elapsed().as_millis(),
        suspends.len(),
        duration(suspends.get(suspends.len() / 2).copied().unwrap_or(0)),
        duration(fork_ns)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guest_reaches_a_model_on_the_host() {
        let url = "http://localhost:8080";
        assert_eq!(
            guest_endpoint(url, Vmm::QemuMicrovm).unwrap(),
            "10.0.2.2:8080"
        );
        assert_eq!(
            guest_endpoint(url, Vmm::Firecracker).unwrap(),
            "172.16.0.1:8080"
        );
        assert_eq!(
            guest_endpoint("http://127.0.0.1:11434/v1", Vmm::CloudHypervisor).unwrap(),
            "172.16.1.1:11434"
        );
        assert_eq!(
            guest_endpoint("http://192.168.1.20:8080", Vmm::QemuVirt).unwrap(),
            "192.168.1.20:8080"
        );
        assert!(guest_endpoint("http://model.lan:8080", Vmm::QemuVirt).is_err());
        assert!(guest_endpoint("http://127.0.0.1", Vmm::QemuVirt).is_err());
    }

    #[test]
    fn turns_and_the_decision_point_are_recognized() {
        assert!(ends_turn(
            "[turn 2] tool: value=4050, 1 notes (python and tool 60.9 ms)",
            true
        ));
        assert!(ends_turn(
            "[turn 1] no tool call: asked again (python 7.6 ms)",
            true
        ));
        assert!(!ends_turn(
            "[turn 1] model (11.9 ms): calls compute(n=900, seed=3)",
            true
        ));
        assert!(ends_turn(
            "turn 1 call_37dd1708 n=804 seed=565 value=209594",
            false
        ));
        assert!(asks_for_fork(
            "[agent] decision point: asking for a fork",
            true
        ));
        assert!(asks_for_fork("fork", false));
        assert!(!asks_for_fork("branch 0", false));
    }
}
