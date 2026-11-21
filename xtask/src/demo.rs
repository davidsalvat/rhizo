//! `cargo xtask demo agent` (design D6 of `add-control-plane`): a thin
//! wrapper around the `rhizo` tool, which runs the CPython agent of
//! `add-agent-workload` against a real model on the host (Ollama,
//! `llama-server`) or any OpenAI-compatible endpoint through the control
//! plane. It builds the control kernel and `rhizo`, then runs `rhizo up`,
//! `rhizo run python-agent` and `rhizo down` with a state of its own. Not
//! run in CI: it needs a model.

use crate::vmm::{Accel, Vmm};
use crate::{Profile, Result, arch, build_kernel, target_dir};
use std::process::Command;

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

/// Runs the agent demo through `rhizo`, console on the terminal.
pub fn cmd(args: &[String]) -> Result {
    let opts = parse(args)?;
    if std::env::var_os("RHIZO_CPYTHON_SRC").is_none() {
        return Err(format!(
            "the demo needs the CPython cell: set RHIZO_CPYTHON_SRC to Python-{}.tar.xz \
             (see docs/demo.md)",
            crate::cpython::VERSION
        ));
    }
    let kernel = build_kernel(Profile::Release, &["control"])?;
    if !crate::cpython::root().join("usr/bin/python3").exists() {
        return Err("CPython was not built (see the build output above)".into());
    }
    let rhizo = crate::control::build_rhizo()?;
    let state = target_dir().join("demo");
    let rhizo_cmd = |words: &[&str]| {
        let mut cmd = Command::new(&rhizo);
        cmd.arg("--state").arg(&state).args(words);
        cmd
    };
    let kernel_path = kernel.to_string_lossy().into_owned();
    let up = [
        "up",
        "--vmm",
        opts.vmm.id(),
        "--arch",
        arch::current().id(),
        "--accel",
        opts.accel.id(),
        "--kernel",
        &kernel_path,
    ];
    let run = [
        "run",
        "python-agent",
        "--llm",
        &opts.llm,
        "--model",
        &opts.model,
    ];
    let status = |mut cmd: Command| {
        cmd.status()
            .map_err(|e| format!("cannot run {}: {e}", rhizo.display()))
            .map(|s| s.success())
    };
    if !status(rhizo_cmd(&up))? {
        return Err("`rhizo up` failed".into());
    }
    let ran = status(rhizo_cmd(&run));
    let _ = status(rhizo_cmd(&["down"]));
    match ran? {
        true => Ok(()),
        false => Err("the demo failed (see above)".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
