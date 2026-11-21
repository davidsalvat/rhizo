//! `cargo xtask demo agent` (design D7 of `add-agent-workload`): the
//! CPython agent in a Rhizo cell against a real model on the host (Ollama,
//! `llama-server`) or any OpenAI-compatible endpoint, its steps shown live.
//! Not run in CI: it needs a model.

use crate::vmm::{self, Accel, Bench, Guest, RunOptions, Vmm};
use crate::{Profile, Result, arch, build_kernel, hostnet};
use std::time::Duration;

/// The options of `demo agent`.
#[derive(Debug, PartialEq)]
struct Options {
    /// The model's base URL as the host sees it (`http://HOST:PORT`).
    llm: String,
    model: String,
    vmm: Vmm,
    accel: Accel,
}

fn parse(args: &[String]) -> Result<Options> {
    let mut it = args.iter();
    if it.next().map(String::as_str) != Some("agent") {
        return Err(
            "usage: cargo xtask demo agent --llm http://HOST:PORT --model NAME \
                    [--vmm <name>] [--accel <auto|kvm|tcg>]"
                .into(),
        );
    }
    let (mut llm, mut model) = (None, None);
    let mut opts = Options {
        llm: String::new(),
        model: String::new(),
        vmm: Vmm::default_for(arch::current()),
        accel: Accel::Auto,
    };
    while let Some(arg) = it.next() {
        let value = it
            .next()
            .cloned()
            .ok_or_else(|| format!("{arg} needs a value"))?;
        match arg.as_str() {
            "--llm" => llm = Some(value),
            "--model" => model = Some(value),
            "--vmm" => opts.vmm = Vmm::parse(&value)?,
            "--accel" => opts.accel = Accel::parse(&value)?,
            other => return Err(format!("unknown option `{other}` for `demo agent`")),
        }
    }
    opts.llm = llm.ok_or("`demo agent` needs --llm http://HOST:PORT")?;
    opts.model = model.ok_or("`demo agent` needs --model NAME")?;
    if opts.model.is_empty() || opts.model.contains(char::is_whitespace) {
        return Err("the model's name cannot be empty or hold spaces".into());
    }
    Ok(opts)
}

/// `IP:PORT` of the model as a guest on `vmm` reaches it: the host's own
/// address there for a model on the host's loopback (or listening on every
/// address), any other IPv4 address as it is.
fn guest_endpoint(url: &str, vmm: Vmm) -> Result<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let hostport = rest.split('/').next().unwrap_or_default();
    let (host, port) = hostport
        .rsplit_once(':')
        .ok_or_else(|| format!("`{url}` names no port (http://HOST:PORT)"))?;
    let port: u16 = port
        .parse()
        .map_err(|_| format!("`{port}` is not a port"))?;
    let host = match host {
        "localhost" | "127.0.0.1" | "0.0.0.0" => hostnet::host_address(vmm),
        ip if ip.parse::<std::net::Ipv4Addr>().is_ok() => ip,
        other => return Err(format!("`{other}`: give the model's IPv4 address")),
    };
    Ok(format!("{host}:{port}"))
}

/// Boots a benchmark kernel that runs the agent demo against `--llm`,
/// console on the terminal.
pub fn cmd(args: &[String]) -> Result {
    let opts = parse(args)?;
    if std::env::var_os("RHIZO_CPYTHON_SRC").is_none() {
        return Err(format!(
            "the demo needs the CPython cell: set RHIZO_CPYTHON_SRC to Python-{}.tar.xz \
             (see docs/demo.md)",
            crate::cpython::VERSION
        ));
    }
    let llm = guest_endpoint(&opts.llm, opts.vmm)?;
    let kernel = build_kernel(Profile::Release, &["bench-cell"])?;
    if !crate::cpython::root().join("usr/bin/python3").exists() {
        return Err("CPython was not built (see the build output above)".into());
    }
    println!(
        "[xtask] agent demo on {} ({}): model `{}` at {llm} as the guest sees it",
        opts.vmm.id(),
        arch::current().id(),
        opts.model
    );
    let run = RunOptions {
        stop_at_ready: false,
        // A real model on a CPU, or the kernel under emulation, can be
        // silent for long.
        timeout: Duration::from_secs(1800),
        echo: true,
        accel: opts.accel,
        gdb: false,
        network: true,
        pci_card: false,
        cpus: 1,
        memory_mib: vmm::DEFAULT_MEMORY_MIB,
        bench: Some(Bench::AgentDemo {
            llm: Box::leak(llm.into_boxed_str()),
            model: Box::leak(opts.model.into_boxed_str()),
        }),
        shard: None,
    };
    let out = vmm::run(opts.vmm, &Guest::Rhizo(kernel), run)?;
    outcome(&out.output)
}

/// The demo's result: its `rhizo-demo done` or `rhizo-demo failed` line
/// (the kernel also prints a `rhizo-demo agent, model …` header first).
fn outcome(output: &str) -> Result {
    let result = output
        .lines()
        .find(|l| l.starts_with("rhizo-demo done") || l.starts_with("rhizo-demo failed"));
    match result {
        Some(line) if line.starts_with("rhizo-demo done") => Ok(()),
        Some(line) => Err(line.to_string()),
        None => Err("the kernel printed no demo result".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_result_line_decides_not_the_header() {
        let header = "rhizo-demo agent, model `qwen2.5:1.5b`\n";
        assert!(
            outcome(&format!(
                "{header}turn 1\nrhizo-demo done in 19600 ms: 3 suspend\n"
            ))
            .is_ok()
        );
        assert_eq!(
            outcome(&format!("{header}rhizo-demo failed after 5 ms: no model\n")),
            Err("rhizo-demo failed after 5 ms: no model".to_string())
        );
        assert!(outcome(header).is_err(), "no result line is a failure");
    }

    #[test]
    fn options_need_a_model_and_its_address() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let o = parse(&args(&[
            "agent",
            "--llm",
            "http://127.0.0.1:11434",
            "--model",
            "qwen2.5:1.5b",
            "--vmm",
            "firecracker",
        ]))
        .unwrap();
        assert_eq!(
            (o.llm.as_str(), o.model.as_str(), o.vmm),
            ("http://127.0.0.1:11434", "qwen2.5:1.5b", Vmm::Firecracker)
        );
        assert!(parse(&args(&["agent", "--model", "m"])).is_err());
        assert!(parse(&args(&["agent", "--llm", "http://h:1"])).is_err());
        assert!(parse(&args(&["other"])).is_err());
        assert!(parse(&args(&["agent", "--llm", "x", "--model", "a b"])).is_err());
    }

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
}
