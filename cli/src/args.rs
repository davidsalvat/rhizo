//! The command line of `rhizo`.

use std::path::PathBuf;
use xtask::arch::Arch;
use xtask::vmm::Accel;

pub const USAGE: &str = "\
Usage: rhizo [--state DIR] <command> [options]

Commands:
  up [options]               Boot a Rhizo VM with the control kernel
      --vmm <name>           qemu (default: qemu-microvm on x86_64, qemu-virt
                             on AArch64), qemu-q35, firecracker,
                             cloud-hypervisor
      --arch <a>             aarch64 or x86_64 (default: the host's)
      --memory <MiB>         Memory of the VM (default 256)
      --cpus <n>             Virtual CPUs (default 1)
      --accel <auto|kvm|tcg> QEMU acceleration (default auto)
      --kernel <path>        A control kernel already built (default: build
                             it with `cargo xtask`)
  run <template> [options] [-- args...]
                             Start a cell and show its output until it ends
      --detach               Print its identifier and return at once
      --file <path>=<value>  A file the cell starts with (@<host file> for
                             a file's contents)
      --env <name>=<value>   An environment variable
                             Templates: python-agent (see below), python
                             (CPython with its standard library; args go to
                             python3), or any program the kernel holds
                             (hello, idle, ...)
  run python-agent --llm <url> --model <name> [--turns <n>] [--transcript]
                             The CPython agent against an OpenAI-compatible
                             model: suspended and resumed after every turn,
                             forked into 3 branches at its decision point
  ps                         List the cells
  stats                      Memory and cells of the VM
  fork <id> [n]              Fork a running cell into n (default 1)
  suspend <id>               Suspend a running cell into a snapshot
  resume <id>                Resume a suspended cell
  attach <id>                Show a cell's output until it ends
  kill <id>                  End a cell, or forget one that ended
  down                       Power the VM off

Every command prints the time the kernel took and the round trip from the
host. The state (VM, control socket, console log) lives in --state, or
$RHIZO_STATE, or ~/.rhizo.
";

/// Options of `up` (and of the background server it starts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Up {
    pub vmm: Option<String>,
    pub arch: Arch,
    pub memory_mib: u32,
    pub cpus: u32,
    pub accel: Accel,
    pub kernel: Option<PathBuf>,
}

/// Contents of a `--file`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileValue {
    Text(String),
    /// A file on the host.
    Host(PathBuf),
}

/// Options of `run`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Run {
    pub template: String,
    pub args: Vec<String>,
    pub files: Vec<(String, FileValue)>,
    pub env: Vec<String>,
    pub detach: bool,
    pub llm: Option<String>,
    pub model: Option<String>,
    pub turns: u32,
    pub transcript: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Up(Up),
    /// The background server `up` starts (not for users).
    Serve(Up),
    Run(Run),
    Ps,
    Stats,
    Fork {
        id: u32,
        n: u32,
    },
    Suspend(u32),
    Resume(u32),
    Attach(u32),
    Kill(u32),
    Down,
    Help,
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub state: Option<PathBuf>,
    pub command: Command,
}

/// The host's architecture, the kernel's by default (CPython is built
/// natively).
pub fn host_arch() -> Arch {
    if std::env::consts::ARCH == "aarch64" {
        Arch::Aarch64
    } else {
        Arch::X86_64
    }
}

fn number<T: std::str::FromStr>(what: &str, value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{what}: `{value}` is not a number"))
}

fn id(args: &[String], command: &str) -> Result<u32, String> {
    match args {
        [id] => number("cell", id),
        _ => Err(format!("usage: rhizo {command} <id>")),
    }
}

/// Parses the arguments after the program's name.
pub fn parse(args: &[String]) -> Result<Args, String> {
    let mut state = None;
    let mut rest = args;
    while let [flag, value, tail @ ..] = rest {
        if flag != "--state" {
            break;
        }
        state = Some(PathBuf::from(value));
        rest = tail;
    }
    let Some((name, args)) = rest.split_first() else {
        return Ok(Args {
            state,
            command: Command::Help,
        });
    };
    let command = match name.as_str() {
        "up" => Command::Up(parse_up(args)?),
        "__serve" => Command::Serve(parse_up(args)?),
        "run" => Command::Run(parse_run(args)?),
        "ps" | "list" => Command::Ps,
        "stats" => Command::Stats,
        "fork" => match args {
            [id] => Command::Fork {
                id: number("cell", id)?,
                n: 1,
            },
            [id, n] => Command::Fork {
                id: number("cell", id)?,
                n: number("count", n)?,
            },
            _ => return Err("usage: rhizo fork <id> [n]".into()),
        },
        "suspend" => Command::Suspend(id(args, "suspend")?),
        "resume" => Command::Resume(id(args, "resume")?),
        "attach" => Command::Attach(id(args, "attach")?),
        "kill" => Command::Kill(id(args, "kill")?),
        "down" => Command::Down,
        "help" | "--help" | "-h" => Command::Help,
        other => return Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    Ok(Args { state, command })
}

fn parse_up(args: &[String]) -> Result<Up, String> {
    let mut up = Up {
        vmm: None,
        arch: host_arch(),
        memory_mib: 256,
        cpus: 1,
        accel: Accel::Auto,
        kernel: None,
    };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--vmm" => up.vmm = Some(value.clone()),
            "--arch" => up.arch = Arch::parse(value)?,
            "--memory" => up.memory_mib = number("--memory", value)?,
            "--cpus" => up.cpus = number("--cpus", value)?,
            "--accel" => up.accel = Accel::parse(value)?,
            "--kernel" => up.kernel = Some(PathBuf::from(value)),
            other => return Err(format!("unknown option `{other}` for `up`")),
        }
    }
    if up.memory_mib < 64 || up.cpus == 0 {
        return Err("the VM needs at least 64 MiB and one CPU".into());
    }
    Ok(up)
}

fn parse_run(args: &[String]) -> Result<Run, String> {
    let mut run = Run {
        turns: 3,
        ..Run::default()
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match arg.as_str() {
            "--" => {
                run.args.extend(it.by_ref().cloned());
                break;
            }
            "--detach" | "-d" => run.detach = true,
            "--transcript" => run.transcript = true,
            "--llm" => run.llm = Some(value("--llm")?),
            "--model" => run.model = Some(value("--model")?),
            "--turns" => run.turns = number("--turns", &value("--turns")?)?,
            "--env" => {
                let env = value("--env")?;
                if !env.contains('=') {
                    return Err(format!("--env {env}: expected NAME=VALUE"));
                }
                run.env.push(env);
            }
            "--file" => {
                let file = value("--file")?;
                let (path, contents) = file
                    .split_once('=')
                    .ok_or_else(|| format!("--file {file}: expected PATH=VALUE"))?;
                if !path.starts_with('/') {
                    return Err(format!("--file {file}: the path must be absolute"));
                }
                let contents = match contents.strip_prefix('@') {
                    Some(host) => FileValue::Host(PathBuf::from(host)),
                    None => FileValue::Text(contents.into()),
                };
                run.files.push((path.into(), contents));
            }
            flag if flag.starts_with("--") => {
                return Err(format!("unknown option `{flag}` for `run`"));
            }
            word if run.template.is_empty() => run.template = word.into(),
            word => run.args.push(word.into()),
        }
    }
    if run.template.is_empty() {
        return Err("usage: rhizo run <template> [options] [-- args...]".into());
    }
    if run.template == "python-agent" {
        if run.llm.is_none() || run.model.is_none() {
            return Err("python-agent needs --llm http://HOST:PORT and --model NAME".into());
        }
        let model = run.model.as_deref().unwrap_or_default();
        if model.is_empty() || model.contains(char::is_whitespace) {
            return Err("the model's name cannot be empty or hold spaces".into());
        }
        if run.turns == 0 {
            return Err("--turns must be at least 1".into());
        }
    } else if run.llm.is_some() || run.model.is_some() || run.transcript {
        return Err("--llm, --model and --transcript are for python-agent".into());
    }
    Ok(run)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_words(words: &str) -> Result<Args, String> {
        let args: Vec<String> = words.split_whitespace().map(String::from).collect();
        parse(&args)
    }

    fn command(words: &str) -> Command {
        parse_words(words).expect("parses").command
    }

    #[test]
    fn up_takes_the_vm_options() {
        let Command::Up(up) = command(
            "up --vmm firecracker --arch x86_64 --memory 512 --cpus 2 --accel tcg --kernel /k",
        ) else {
            panic!("not up");
        };
        assert_eq!(up.vmm.as_deref(), Some("firecracker"));
        assert_eq!(up.arch, Arch::X86_64);
        assert_eq!((up.memory_mib, up.cpus), (512, 2));
        assert_eq!(up.accel, Accel::Tcg);
        assert_eq!(up.kernel, Some(PathBuf::from("/k")));
        let Command::Up(defaults) = command("up") else {
            panic!("not up");
        };
        assert_eq!((defaults.vmm, defaults.arch), (None, host_arch()));
        assert_eq!((defaults.memory_mib, defaults.cpus), (256, 1));
        assert!(parse_words("up --cpus 0").is_err());
        assert!(parse_words("up --memory lots").is_err());
        assert!(parse_words("up --vmm").is_err());
        assert!(parse_words("up --frobnicate 1").is_err());
    }

    #[test]
    fn the_state_comes_first() {
        let args = parse_words("--state /tmp/r ps").unwrap();
        assert_eq!(args.state, Some(PathBuf::from("/tmp/r")));
        assert_eq!(args.command, Command::Ps);
        assert_eq!(parse_words("").unwrap().command, Command::Help);
    }

    #[test]
    fn cell_commands_take_identifiers() {
        assert_eq!(command("fork 3"), Command::Fork { id: 3, n: 1 });
        assert_eq!(command("fork 3 4"), Command::Fork { id: 3, n: 4 });
        assert_eq!(command("suspend 7"), Command::Suspend(7));
        assert_eq!(command("resume 7"), Command::Resume(7));
        assert_eq!(command("attach 7"), Command::Attach(7));
        assert_eq!(command("kill 7"), Command::Kill(7));
        assert_eq!(command("down"), Command::Down);
        assert_eq!(command("list"), Command::Ps);
        assert!(parse_words("kill").is_err());
        assert!(parse_words("kill x").is_err());
        assert!(parse_words("fork 1 2 3").is_err());
        assert!(parse_words("teleport 1").is_err());
    }

    #[test]
    fn run_takes_files_environment_and_arguments() {
        let Command::Run(run) = command(
            "run python --file /app/x.py=print(1) --file /data=@notes.txt --env A=1 -d -- -c x",
        ) else {
            panic!("not run");
        };
        assert_eq!(run.template, "python");
        assert_eq!(run.args, ["-c", "x"]);
        assert_eq!(
            run.files,
            [
                ("/app/x.py".into(), FileValue::Text("print(1)".into())),
                ("/data".into(), FileValue::Host("notes.txt".into())),
            ]
        );
        assert_eq!(run.env, ["A=1"]);
        assert!(run.detach);
        let Command::Run(hello) = command("run hello world") else {
            panic!("not run");
        };
        assert_eq!(
            (hello.template.as_str(), hello.args.as_slice()),
            ("hello", &["world".to_string()][..])
        );
        assert!(parse_words("run").is_err());
        assert!(parse_words("run x --file relative=1").is_err());
        assert!(parse_words("run x --file /nothing").is_err());
        assert!(parse_words("run x --env NOVALUE").is_err());
        assert!(parse_words("run x --bogus").is_err());
    }

    #[test]
    fn the_agent_needs_a_model_and_its_address() {
        let Command::Run(run) =
            command("run python-agent --llm http://127.0.0.1:11434 --model qwen2.5:1.5b --turns 5")
        else {
            panic!("not run");
        };
        assert_eq!(run.llm.as_deref(), Some("http://127.0.0.1:11434"));
        assert_eq!(run.model.as_deref(), Some("qwen2.5:1.5b"));
        assert_eq!((run.turns, run.transcript), (5, false));
        assert!(parse_words("run python-agent --model m").is_err());
        assert!(parse_words("run python-agent --llm http://h:1").is_err());
        assert!(parse_words("run python-agent --llm http://h:1 --model m --turns 0").is_err());
        assert!(parse_words("run hello --llm http://h:1").is_err());
    }
}
