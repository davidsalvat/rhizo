//! The command line of `rhizo`.

use crate::tree::Place;
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
                             Placement (protocol 4):
      --parent <id>          Place it under this cell (default: a root)
      --label <name>         Its label ([A-Za-z0-9._-], at most 64 bytes)
      --egress <policy>      none (default), model, full, or rules
                             PROTO:CIDR:PORTS (tcp:140.82.112.0/20:443),
                             several joined by commas
      --ceiling <policy>     Its exposure ceiling (default: its egress)
      --mem <size>           Memory limit (256M, 2G)
      --cpu-time <time>      CPU time limit (90s, 500ms)
      --sockets <n>          Socket limit
      --cells <n>            Descendant limit (its children need one)
      --on-parent-end <p>    cascade (default) or promote
      --cpus <set>           An exclusive CPU set, roots only (2-3,5)
      --cpu-weight <n>       CPU weight among its siblings (1 to 10000)
      --cpu-max <cpus>       CPU cap of its subtree (1.5)
      --llm <url>            The model endpoint `--egress model` names
  run python-agent --llm <url> --model <name> [--turns <n>] [--transcript]
                             The CPython agent against an OpenAI-compatible
                             model: suspended and resumed after every turn,
                             forked into 3 branches at its decision point
  chat --llm <url> --model <name>
                             Talk to a new agent in a cell (CPython with
                             tools that run in the cell); /help inside
  chat <id>                  Talk to it again, resuming it if suspended
  ps [--tree]                List the cells (--tree: under their parents,
                             with labels, egress and limits)
  stats                      Memory and cells of the VM
  fork <id> [n] [options]    Fork a running cell into n (default 1), or
                             make n cells of a template, placed as `run`'s
  suspend <id> [--tree]      Suspend a running cell (--tree: its subtree)
  resume <id> [--tree]       Resume a suspended cell (--tree: its subtree)
  freeze <id>                Freeze a running cell into a template, which
                             `fork` makes cells from
  save <id> <file>           Save a suspended cell or a template to a file
  load <file> [--template] [options]
                             Load a saved file as a suspended cell, or as a
                             template, into this VM, placed as `run`'s
  attach <id>                Show a cell's output until it ends
  kill <id> [--tree]         End a cell, or forget one that ended (--tree:
                             its whole subtree)
  reparent <id> --to <id|root>
                             Move a cell under another, or make it a root
  policy <id> [--egress <p>] [--ceiling <p>] [--mem ...]
                             Narrow or set a cell's egress and limits
  scope <id> --socket <path> Serve on <path> requests limited to the cell's
                             subtree, until interrupted
  audit tail [--cell <id>] [--domain <id>] [--kind <k>] [--follow]
                             Show the audit log: what the kernel refused and
                             what was administered (kinds: egress-denied,
                             scope-denied, tree-denied, load-narrowed,
                             publish-refused, auth-refused, created, ended,
                             moved, policy, limits, publish-attested,
                             scoped)
  audit verify               Check the audit log's hash chain: edited,
                             removed, reordered and missing records
  down                       Power the VM off
  snapshot upgrade <in> <out>
                             Convert a file saved by an older kernel (one
                             format version before) to the current version

Every command prints the time the kernel took and the round trip from the
host. The state (VM, control socket, console log) lives in --state, or
$RHIZO_STATE, or ~/.rhizo; after `down` only the last console log and the
audit log (audit/) stay.
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

/// What `chat` talks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chat {
    /// A new chat cell against this model.
    New { llm: String, model: String },
    /// An existing chat cell.
    Cell(u32),
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
    /// Where the cell goes in the tree (protocol 4).
    pub place: Place,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Up(Up),
    /// The background server `up` starts (not for users).
    Serve(Up),
    Run(Run),
    Chat(Chat),
    Ps,
    /// `ps --tree`.
    PsTree,
    Stats,
    Fork {
        id: u32,
        n: u32,
        place: Place,
    },
    Suspend(u32),
    SuspendTree(u32),
    Resume(u32),
    ResumeTree(u32),
    Freeze(u32),
    Save {
        id: u32,
        file: PathBuf,
    },
    Load {
        file: PathBuf,
        template: bool,
        place: Place,
    },
    Attach(u32),
    Kill(u32),
    KillTree(u32),
    /// Moves a cell under `to` (0: makes it a root).
    Reparent {
        id: u32,
        to: u32,
    },
    /// Sets a cell's egress, ceiling and limits (those given).
    Policy {
        id: u32,
        place: Place,
    },
    /// Serves requests limited to a cell's subtree on a socket.
    Scope {
        id: u32,
        socket: PathBuf,
    },
    Down,
    /// `audit tail`: shows the records `filter` keeps, then, with
    /// `follow`, new ones as they come.
    AuditTail {
        filter: crate::audit::Filter,
        follow: bool,
    },
    /// `audit verify`: walks the log's hash chain.
    AuditVerify,
    /// `snapshot upgrade IN OUT`: converts a saved file to the current
    /// format version (no VM needed).
    SnapshotUpgrade {
        input: PathBuf,
        output: PathBuf,
    },
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

/// A cell, with or without `--tree`.
fn id_tree(args: &[String], command: &str) -> Result<(u32, bool), String> {
    match args {
        [id] => Ok((number("cell", id)?, false)),
        [id, flag] | [flag, id] if flag == "--tree" => Ok((number("cell", id)?, true)),
        _ => Err(format!("usage: rhizo {command} <id> [--tree]")),
    }
}

/// The words of `args` that are not options, its placement options, and
/// whether each of `switches` (options without a value) was given.
fn split(
    args: &[String],
    switches: &[&str],
    command: &str,
) -> Result<(Vec<String>, Place, Vec<bool>), String> {
    let (mut words, mut place) = (Vec::new(), Place::default());
    let mut on = vec![false; switches.len()];
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if let Some(k) = switches.iter().position(|s| s == arg) {
            on[k] = true;
        } else if arg.starts_with("--") {
            let value = it.next().ok_or_else(|| format!("{arg} needs a value"))?;
            if !place.option(arg, value)? {
                return Err(format!("unknown option `{arg}` for `{command}`"));
            }
        } else {
            words.push(arg.clone());
        }
    }
    Ok((words, place, on))
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
        "chat" => Command::Chat(parse_chat(args)?),
        "ps" | "list" => match args {
            [] => Command::Ps,
            [flag] if flag == "--tree" => Command::PsTree,
            _ => return Err("usage: rhizo ps [--tree]".into()),
        },
        "stats" => Command::Stats,
        "fork" => {
            let (words, place, _) = split(args, &[], "fork")?;
            match words.as_slice() {
                [id] => Command::Fork {
                    id: number("cell", id)?,
                    n: 1,
                    place,
                },
                [id, n] => Command::Fork {
                    id: number("cell", id)?,
                    n: number("count", n)?,
                    place,
                },
                _ => return Err("usage: rhizo fork <id> [n] [options]".into()),
            }
        }
        "suspend" => match id_tree(args, "suspend")? {
            (id, false) => Command::Suspend(id),
            (id, true) => Command::SuspendTree(id),
        },
        "resume" => match id_tree(args, "resume")? {
            (id, false) => Command::Resume(id),
            (id, true) => Command::ResumeTree(id),
        },
        "freeze" => Command::Freeze(id(args, "freeze")?),
        "save" => match args {
            [id, file] => Command::Save {
                id: number("cell", id)?,
                file: PathBuf::from(file),
            },
            _ => return Err("usage: rhizo save <id> <file>".into()),
        },
        "load" => {
            let (words, place, on) = split(args, &["--template"], "load")?;
            match words.as_slice() {
                [file] => Command::Load {
                    file: PathBuf::from(file),
                    template: on[0],
                    place,
                },
                _ => return Err("usage: rhizo load <file> [--template] [options]".into()),
            }
        }
        "attach" => Command::Attach(id(args, "attach")?),
        "kill" => match id_tree(args, "kill")? {
            (id, false) => Command::Kill(id),
            (id, true) => Command::KillTree(id),
        },
        "reparent" => match args {
            [id, flag, to] if flag == "--to" => Command::Reparent {
                id: number("cell", id)?,
                to: match to.as_str() {
                    "root" => 0,
                    to => number("cell", to)?,
                },
            },
            _ => return Err("usage: rhizo reparent <id> --to <id|root>".into()),
        },
        "policy" => {
            let (words, place, _) = split(args, &[], "policy")?;
            let only = Place {
                egress: place.egress.clone(),
                ceiling: place.ceiling.clone(),
                memory: place.memory,
                cpu_ns: place.cpu_ns,
                sockets: place.sockets,
                cells: place.cells,
                llm: place.llm.clone(),
                ..Place::default()
            };
            match words.as_slice() {
                [id] if only == place && (place.egress.is_some() || place.has_limits()) => {
                    if place.ceiling.is_some() && place.egress.is_none() {
                        return Err("--ceiling goes with --egress".into());
                    }
                    Command::Policy {
                        id: number("cell", id)?,
                        place,
                    }
                }
                _ => {
                    return Err("usage: rhizo policy <id> [--egress <p>] [--ceiling <p>] \
                                [--mem <size>] [--cpu-time <t>] [--sockets <n>] [--cells <n>]"
                        .into());
                }
            }
        }
        "scope" => match args {
            [id, flag, path] if flag == "--socket" => Command::Scope {
                id: number("cell", id)?,
                socket: PathBuf::from(path),
            },
            _ => return Err("usage: rhizo scope <id> --socket <path>".into()),
        },
        "down" => Command::Down,
        "audit" => parse_audit(args)?,
        "snapshot" => match args {
            [sub, input, output] if sub == "upgrade" => Command::SnapshotUpgrade {
                input: PathBuf::from(input),
                output: PathBuf::from(output),
            },
            _ => return Err("usage: rhizo snapshot upgrade <in> <out>".into()),
        },
        "help" | "--help" | "-h" => Command::Help,
        other => return Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    Ok(Args { state, command })
}

fn parse_audit(args: &[String]) -> Result<Command, String> {
    const USAGE: &str = "usage: rhizo audit tail [--cell <id>] [--domain <id>] [--kind <k>] \
                         [--follow], or rhizo audit verify";
    match args.split_first() {
        Some((sub, [])) if sub == "verify" => Ok(Command::AuditVerify),
        Some((sub, rest)) if sub == "tail" => {
            let (mut filter, mut follow) = (crate::audit::Filter::default(), false);
            let mut it = rest.iter();
            while let Some(flag) = it.next() {
                if flag == "--follow" || flag == "-f" {
                    follow = true;
                    continue;
                }
                let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
                match flag.as_str() {
                    "--cell" => filter.cell = Some(number("--cell", value)?),
                    "--domain" => filter.domain = Some(number("--domain", value)?),
                    "--kind" => {
                        filter.kind = Some(
                            rhizo_control::audit::Kind::parse(value)
                                .ok_or_else(|| format!("--kind: no kind `{value}`"))?,
                        );
                    }
                    other => return Err(format!("unknown option `{other}`; {USAGE}")),
                }
            }
            Ok(Command::AuditTail { filter, follow })
        }
        _ => Err(USAGE.into()),
    }
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

fn parse_chat(args: &[String]) -> Result<Chat, String> {
    const USAGE: &str = "usage: rhizo chat --llm http://HOST:PORT --model NAME, or rhizo chat <id>";
    let (mut llm, mut model) = (None, None);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match arg.as_str() {
            "--llm" => llm = Some(value("--llm")?),
            "--model" => model = Some(value("--model")?),
            id if args.len() == 1 => return Ok(Chat::Cell(number("cell", id)?)),
            other => return Err(format!("unexpected `{other}`; {USAGE}")),
        }
    }
    match (llm, model) {
        (Some(llm), Some(model)) if !model.is_empty() && !model.contains(char::is_whitespace) => {
            Ok(Chat::New { llm, model })
        }
        (Some(_), Some(_)) => Err("the model's name cannot be empty or hold spaces".into()),
        _ => Err(USAGE.into()),
    }
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
                let value = value(flag)?;
                if !run.place.option(flag, &value)? {
                    return Err(format!("unknown option `{flag}` for `run`"));
                }
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
    } else if run.model.is_some() || run.transcript {
        return Err("--model and --transcript are for python-agent".into());
    } else if run.llm.is_some() && !run.place.egress.as_ref().is_some_and(|e| e.model) {
        return Err("--llm is for python-agent, or names the endpoint of --egress model".into());
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
    fn audit_takes_its_filters() {
        assert_eq!(command("audit verify"), Command::AuditVerify);
        assert_eq!(
            command("audit tail --cell 3 --kind scope-denied -f"),
            Command::AuditTail {
                filter: crate::audit::Filter {
                    cell: Some(3),
                    domain: None,
                    kind: Some(rhizo_control::audit::Kind::Scope),
                },
                follow: true,
            }
        );
        assert!(parse_words("audit tail --kind nothing").is_err());
        assert!(parse_words("audit tail --cell").is_err());
        assert!(parse_words("audit verify now").is_err());
        assert!(parse_words("audit").is_err());
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
        let fork = |id, n| Command::Fork {
            id,
            n,
            place: Place::default(),
        };
        assert_eq!(command("fork 3"), fork(3, 1));
        assert_eq!(command("fork 3 4"), fork(3, 4));
        assert_eq!(command("suspend 7"), Command::Suspend(7));
        assert_eq!(command("resume 7"), Command::Resume(7));
        assert_eq!(command("attach 7"), Command::Attach(7));
        assert_eq!(command("freeze 7"), Command::Freeze(7));
        assert_eq!(
            command("save 7 agent.rhz"),
            Command::Save {
                id: 7,
                file: "agent.rhz".into()
            }
        );
        for words in ["load agent.rhz --template", "load --template agent.rhz"] {
            assert_eq!(
                command(words),
                Command::Load {
                    file: "agent.rhz".into(),
                    template: true,
                    place: Place::default(),
                }
            );
        }
        assert_eq!(
            command("load agent.rhz"),
            Command::Load {
                file: "agent.rhz".into(),
                template: false,
                place: Place::default(),
            }
        );
        assert!(parse_words("save 7").is_err());
        assert!(parse_words("save x f").is_err());
        assert!(parse_words("load").is_err());
        assert!(parse_words("load a b").is_err());
        assert_eq!(command("kill 7"), Command::Kill(7));
        assert_eq!(command("down"), Command::Down);
        assert_eq!(
            command("snapshot upgrade old.rhz new.rhz"),
            Command::SnapshotUpgrade {
                input: "old.rhz".into(),
                output: "new.rhz".into()
            }
        );
        assert!(parse_words("snapshot upgrade old.rhz").is_err());
        assert!(parse_words("snapshot downgrade a b").is_err());
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
    fn chat_starts_an_agent_or_talks_to_one() {
        assert_eq!(
            command("chat --llm http://127.0.0.1:8080 --model qwen2.5"),
            Command::Chat(Chat::New {
                llm: "http://127.0.0.1:8080".into(),
                model: "qwen2.5".into()
            })
        );
        assert_eq!(command("chat 12"), Command::Chat(Chat::Cell(12)));
        assert!(parse_words("chat").is_err());
        assert!(parse_words("chat x").is_err());
        assert!(parse_words("chat 1 2").is_err());
        assert!(parse_words("chat --llm http://h:1").is_err());
        assert!(parse_words("chat --model m").is_err());
        assert!(parse_words("chat --llm http://h:1 --model").is_err());
        assert!(parse_words("chat --llm http://h:1 --model m 3").is_err());
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

    /// The tree commands of protocol 4 (spec `control-plane`, tree under
    /// control and tree operations from the host).
    #[test]
    fn tree_commands_parse() {
        assert_eq!(command("ps --tree"), Command::PsTree);
        assert!(parse_words("ps --forest").is_err());
        assert_eq!(command("kill 3 --tree"), Command::KillTree(3));
        assert_eq!(command("suspend --tree 3"), Command::SuspendTree(3));
        assert_eq!(command("resume 3 --tree"), Command::ResumeTree(3));
        assert!(parse_words("kill 3 --all").is_err());
        assert_eq!(
            command("reparent 7 --to 3"),
            Command::Reparent { id: 7, to: 3 }
        );
        assert_eq!(
            command("reparent 7 --to root"),
            Command::Reparent { id: 7, to: 0 }
        );
        assert!(parse_words("reparent 7 3").is_err());
        assert_eq!(
            command("scope 3 --socket ./agent3.sock"),
            Command::Scope {
                id: 3,
                socket: "./agent3.sock".into()
            }
        );
        assert!(parse_words("scope 3").is_err());
        let Command::Policy { id: 7, place } =
            command("policy 7 --egress allow:tcp:140.82.112.0/20:443 --mem 128M")
        else {
            panic!("not a policy");
        };
        assert_eq!(place.memory, Some(128 << 20));
        assert_eq!(place.egress.map(|e| e.rules.len()), Some(1));
        // Nothing to set, a placement option, or a ceiling alone.
        assert!(parse_words("policy 7").is_err());
        assert!(parse_words("policy 7 --parent 3").is_err());
        assert!(parse_words("policy 7 --ceiling none").is_err());
    }

    #[test]
    fn placement_options_go_with_run_fork_and_load() {
        let Command::Run(run) = command(
            "run python --parent 3 --label tools --egress model --llm http://127.0.0.1:8080 \
             --mem 256M --cells 4 --on-parent-end cascade -- -c pass",
        ) else {
            panic!("not a run");
        };
        assert_eq!(run.place.parent, Some(3));
        assert_eq!(run.place.label.as_deref(), Some("tools"));
        assert_eq!(run.place.cells, Some(4));
        assert_eq!(run.args, ["-c", "pass"]);
        // `--llm` names the endpoint of `--egress model`, else it is the
        // agent's.
        assert!(parse_words("run python --llm http://h:1").is_err());
        let Command::Fork { id: 5, n: 2, place } = command("fork 5 2 --parent 3 --mem 64M") else {
            panic!("not a fork");
        };
        assert_eq!((place.parent, place.memory), (Some(3), Some(64 << 20)));
        let Command::Load {
            template: true,
            place,
            ..
        } = command("load agent.rhz --template --egress none --cpu-weight 50")
        else {
            panic!("not a load");
        };
        assert_eq!(place.cpu_weight, Some(50));
        assert!(parse_words("run idle --cpus 64").is_err());
        assert!(parse_words("fork 5 --mem").is_err());
    }
}
