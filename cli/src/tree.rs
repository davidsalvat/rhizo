//! The cell tree from the command line (protocol version 4, design D14 of
//! `add-cell-hierarchy`): the placement options of `run`, `fork` and
//! `load`, egress policies as people write them, and `rhizo ps --tree`.

use rhizo_control::{CellInfo, Egress, Limits, OnParentEnd, Placement, Proto, Rule};
use std::collections::HashMap;

/// An egress policy as given: `none`, `full`, `model` (the model endpoint
/// as the guest reaches it, from `--llm`), or rules `PROTO:CIDR:PORTS`
/// (`tcp`, `udp` or `any`; one port or `LO-HI`; `allow:` in front is
/// optional), several joined by commas or `--egress` given again.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EgressArg {
    pub full: bool,
    pub model: bool,
    pub rules: Vec<Rule>,
}

/// Placement options as given; `None` is not given.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Place {
    pub parent: Option<u32>,
    pub label: Option<String>,
    pub egress: Option<EgressArg>,
    pub ceiling: Option<EgressArg>,
    pub memory: Option<u64>,
    pub cpu_ns: Option<u64>,
    pub sockets: Option<u32>,
    pub cells: Option<u32>,
    pub on_parent_end: Option<OnParentEnd>,
    pub cpus: Option<u64>,
    pub cpu_weight: Option<u16>,
    pub cpu_max: Option<u32>,
    /// `--llm`: the model endpoint `model` names.
    pub llm: Option<String>,
}

impl Place {
    /// Takes `flag` with `value` if it is a placement option; `Ok(false)`
    /// if it is not one.
    pub fn option(&mut self, flag: &str, value: &str) -> Result<bool, String> {
        let n = |v: &str| {
            v.parse::<u32>()
                .map_err(|_| format!("{flag}: `{v}` is not a number"))
        };
        match flag {
            "--parent" => self.parent = Some(n(value)?),
            "--label" => self.label = Some(value.into()),
            "--egress" => egress_arg(&mut self.egress, value)?,
            "--ceiling" => egress_arg(&mut self.ceiling, value)?,
            "--mem" => self.memory = Some(size(value)?),
            "--cpu-time" => self.cpu_ns = Some(time(value)?),
            "--sockets" => self.sockets = Some(n(value)?),
            "--cells" => self.cells = Some(n(value)?),
            "--on-parent-end" => {
                self.on_parent_end = Some(match value {
                    "cascade" => OnParentEnd::Cascade,
                    "promote" => OnParentEnd::Promote,
                    _ => return Err(format!("--on-parent-end {value}: cascade or promote")),
                })
            }
            "--cpus" => self.cpus = Some(cpu_set(value)?),
            "--cpu-weight" => {
                let w = n(value)?;
                if !(1..=10_000).contains(&w) {
                    return Err(format!("--cpu-weight {value}: from 1 to 10000"));
                }
                self.cpu_weight = Some(w as u16);
            }
            "--cpu-max" => self.cpu_max = Some(cpu_max(value)?),
            "--llm" => self.llm = Some(value.into()),
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Whether only defaults were given (version 3 can say it).
    pub fn is_default(&self) -> bool {
        *self
            == Place {
                llm: self.llm.clone(),
                ..Place::default()
            }
    }

    /// The limits given; the others unlimited.
    pub fn limits(&self) -> Limits {
        Limits {
            memory: self.memory.unwrap_or(u64::MAX),
            cpu_ns: self.cpu_ns.unwrap_or(u64::MAX),
            sockets: self.sockets.unwrap_or(u32::MAX),
            cells: self.cells.unwrap_or(u32::MAX),
        }
    }

    /// Whether a limit was given.
    pub fn has_limits(&self) -> bool {
        self.memory.is_some()
            || self.cpu_ns.is_some()
            || self.sockets.is_some()
            || self.cells.is_some()
    }

    /// The placement to send; `model` is the model endpoint's rule, when
    /// one is known.
    pub fn placement(&self, model: Option<Rule>) -> Result<Placement, String> {
        let egress = |e: &Option<EgressArg>| e.as_ref().map(|e| e.resolve(model)).transpose();
        Ok(Placement {
            parent: self.parent.unwrap_or(0),
            label: self.label.clone().unwrap_or_default().into_bytes(),
            limits: self.limits(),
            egress: egress(&self.egress)?.unwrap_or_default(),
            ceiling: egress(&self.ceiling)?,
            on_parent_end: self.on_parent_end.unwrap_or_default(),
            cpus: self.cpus.unwrap_or(0),
            cpu_weight: self.cpu_weight.unwrap_or(0),
            cpu_max: self.cpu_max.unwrap_or(u32::MAX),
        })
    }
}

impl EgressArg {
    /// The policy, with `model` replaced by the model endpoint's rule.
    pub fn resolve(&self, model: Option<Rule>) -> Result<Egress, String> {
        if self.full {
            return Ok(Egress::Full);
        }
        let mut rules = self.rules.clone();
        if self.model {
            rules.push(model.ok_or(
                "`--egress model` names the model endpoint: give it with --llm http://HOST:PORT",
            )?);
        }
        Ok(if rules.is_empty() {
            Egress::None
        } else {
            Egress::Rules(rules)
        })
    }
}

/// Adds `value`'s items to an egress policy being given.
fn egress_arg(into: &mut Option<EgressArg>, value: &str) -> Result<(), String> {
    let e = into.get_or_insert_with(EgressArg::default);
    for item in value.split(',') {
        match item {
            "none" => {}
            "full" => e.full = true,
            "model" => e.model = true,
            rule => e.rules.push(parse_rule(rule)?),
        }
    }
    Ok(())
}

/// `[allow:]PROTO:A.B.C.D[/N]:PORT[-PORT]`.
pub fn parse_rule(text: &str) -> Result<Rule, String> {
    let bad = |why: &str| format!("egress rule `{text}`: {why} (PROTO:CIDR:PORTS)");
    let rule = text.strip_prefix("allow:").unwrap_or(text);
    let mut parts = rule.split(':');
    let (Some(proto), Some(net), Some(ports), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(bad("three parts"));
    };
    let proto = match proto {
        "tcp" => Proto::Tcp,
        "udp" => Proto::Udp,
        "any" => Proto::Any,
        _ => return Err(bad("tcp, udp or any")),
    };
    let (addr, prefix) = net.split_once('/').unwrap_or((net, "32"));
    let addr: std::net::Ipv4Addr = addr.parse().map_err(|_| bad("an IPv4 address"))?;
    let prefix: u8 = prefix
        .parse()
        .ok()
        .filter(|&p| p <= 32)
        .ok_or_else(|| bad("a prefix from 0 to 32"))?;
    let (lo, hi) = ports.split_once('-').unwrap_or((ports, ports));
    let port = |p: &str| p.parse::<u16>().map_err(|_| bad("ports from 0 to 65535"));
    let (lo, hi) = (port(lo)?, port(hi)?);
    if lo > hi {
        return Err(bad("an empty port range"));
    }
    Ok(Rule {
        proto,
        addr: addr.octets(),
        prefix,
        lo,
        hi,
    })
}

/// The rule that lets a cell reach `endpoint` (`IP:PORT`) over TCP.
pub fn endpoint_rule(endpoint: &str) -> Result<Rule, String> {
    parse_rule(&format!("tcp:{endpoint}"))
}

/// `256M`, `2G`, `64K` or bytes.
fn size(text: &str) -> Result<u64, String> {
    let (digits, unit) = match text.char_indices().last() {
        Some((i, c)) if c.is_ascii_alphabetic() => (&text[..i], c.to_ascii_uppercase()),
        _ => (text, 'B'),
    };
    let shift = match unit {
        'B' => 0,
        'K' => 10,
        'M' => 20,
        'G' => 30,
        _ => return Err(format!("--mem {text}: a size such as 256M or 2G")),
    };
    digits
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(1 << shift))
        .ok_or_else(|| format!("--mem {text}: a size such as 256M or 2G"))
}

/// `90s`, `500ms` or `2m`, in nanoseconds.
fn time(text: &str) -> Result<u64, String> {
    let bad = || format!("--cpu-time {text}: a time such as 90s or 500ms");
    let (digits, ns) = if let Some(d) = text.strip_suffix("ms") {
        (d, 1_000_000)
    } else if let Some(d) = text.strip_suffix('s') {
        (d, 1_000_000_000)
    } else if let Some(d) = text.strip_suffix('m') {
        (d, 60_000_000_000)
    } else {
        return Err(bad());
    };
    digits
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(ns))
        .ok_or_else(bad)
}

/// `2-3,5`: one bit per CPU, CPUs 0 to 63.
fn cpu_set(text: &str) -> Result<u64, String> {
    let bad = || format!("--cpus {text}: CPUs from 0 to 63, such as 2-3,5");
    let mut set = 0u64;
    for part in text.split(',') {
        let (lo, hi) = part.split_once('-').unwrap_or((part, part));
        let cpu = |c: &str| c.parse::<u32>().ok().filter(|&c| c < 64).ok_or_else(bad);
        let (lo, hi) = (cpu(lo)?, cpu(hi)?);
        if lo > hi {
            return Err(bad());
        }
        for c in lo..=hi {
            set |= 1 << c;
        }
    }
    Ok(set)
}

/// `1.5` CPUs, in thousandths.
fn cpu_max(text: &str) -> Result<u32, String> {
    text.parse::<f64>()
        .ok()
        .filter(|c| *c > 0.0 && *c <= 4_000_000.0)
        .map(|c| (c * 1000.0).round() as u32)
        .filter(|&m| m > 0)
        .ok_or_else(|| format!("--cpu-max {text}: a number of CPUs, such as 1.5"))
}

/// An egress policy for people.
pub fn show_egress(e: &Egress) -> String {
    match e {
        Egress::None => "none".into(),
        Egress::Full => "full".into(),
        Egress::Rules(rules) => rules
            .iter()
            .map(|r| {
                let proto = match r.proto {
                    Proto::Tcp => "tcp",
                    Proto::Udp => "udp",
                    Proto::Any => "any",
                };
                let [a, b, c, d] = r.addr;
                let ports = if r.lo == r.hi {
                    r.lo.to_string()
                } else {
                    format!("{}-{}", r.lo, r.hi)
                };
                format!("{proto}:{a}.{b}.{c}.{d}/{}:{ports}", r.prefix)
            })
            .collect::<Vec<_>>()
            .join(","),
    }
}

/// Limits for people: the bounded ones.
pub fn show_limits(l: &Limits) -> String {
    let mut out = Vec::new();
    if l.memory != u64::MAX {
        out.push(if l.memory >= 1 << 30 && l.memory.is_multiple_of(1 << 30) {
            format!("mem {}G", l.memory >> 30)
        } else {
            format!("mem {}M", l.memory >> 20)
        });
    }
    if l.cpu_ns != u64::MAX {
        out.push(format!("cpu {}s", l.cpu_ns / 1_000_000_000));
    }
    if l.sockets != u32::MAX {
        out.push(format!("{} sockets", l.sockets));
    }
    if l.cells != u32::MAX {
        out.push(format!("{} cells", l.cells));
    }
    if out.is_empty() {
        "unlimited".into()
    } else {
        out.join(", ")
    }
}

/// The rows of `rhizo ps --tree`: each cell after its parent, indented
/// under it, then the cells outside the tree (ended, or loaded and not yet
/// resumed). `state` describes a cell's state.
pub fn tree_rows(cells: &[CellInfo], state: impl Fn(&CellInfo) -> String) -> Vec<String> {
    let known: HashMap<u32, &CellInfo> = cells.iter().map(|c| (c.id, c)).collect();
    let mut children: HashMap<u32, Vec<&CellInfo>> = HashMap::new();
    let mut roots = Vec::new();
    for c in cells {
        match &c.tree {
            Some(t) if t.parent != 0 && known.contains_key(&t.parent) => {
                children.entry(t.parent).or_default().push(c)
            }
            Some(_) => roots.push(c),
            None => {}
        }
    }
    let mut rows = vec![format!(
        "{:<10} {:<22} {:<10} {:<10} {:<24} LIMITS",
        "ID", "STATE", "IMAGE", "LABEL", "EGRESS"
    )];
    let mut stack: Vec<(&CellInfo, usize)> = roots.iter().rev().map(|&c| (c, 0)).collect();
    while let Some((c, depth)) = stack.pop() {
        let t = c.tree.as_ref().expect("in the tree");
        let id = format!("{}{}", "  ".repeat(depth), c.id);
        rows.push(format!(
            "{id:<10} {:<22} {:<10} {:<10} {:<24} {}",
            state(c),
            String::from_utf8_lossy(&c.image),
            String::from_utf8_lossy(&t.label),
            show_egress(&t.egress),
            show_limits(&t.limits)
        ));
        if let Some(kids) = children.get(&c.id) {
            stack.extend(kids.iter().rev().map(|&k| (k, depth + 1)));
        }
    }
    for c in cells.iter().filter(|c| c.tree.is_none()) {
        rows.push(format!(
            "{:<10} {:<22} {}",
            c.id,
            state(c),
            String::from_utf8_lossy(&c.image)
        ));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhizo_control::{State, TreeInfo};

    fn place(words: &str) -> Result<Place, String> {
        let mut p = Place::default();
        let w: Vec<&str> = words.split_whitespace().collect();
        for pair in w.chunks(2) {
            assert!(p.option(pair[0], pair[1])?, "{}", pair[0]);
        }
        Ok(p)
    }

    #[test]
    fn placement_options_parse() {
        let p = place(
            "--parent 3 --label tools --egress model --mem 256M --cells 4 \
             --on-parent-end promote --cpus 2-3,5 --cpu-weight 250 --cpu-max 1.5 \
             --cpu-time 90s --sockets 8",
        )
        .unwrap();
        let model = endpoint_rule("10.0.2.2:8080").unwrap();
        let placement = p.placement(Some(model)).unwrap();
        assert_eq!(
            placement,
            Placement {
                parent: 3,
                label: b"tools".to_vec(),
                limits: Limits {
                    memory: 256 << 20,
                    cpu_ns: 90_000_000_000,
                    sockets: 8,
                    cells: 4,
                },
                egress: Egress::Rules(vec![Rule {
                    proto: Proto::Tcp,
                    addr: [10, 0, 2, 2],
                    prefix: 32,
                    lo: 8080,
                    hi: 8080,
                }]),
                ceiling: None,
                on_parent_end: OnParentEnd::Promote,
                cpus: 0b10_1100,
                cpu_weight: 250,
                cpu_max: 1500,
            }
        );
        // No option: version 3's root, egress `none`.
        assert!(Place::default().is_default());
        assert_eq!(Place::default().placement(None).unwrap(), Placement::root());
        assert!(!p.is_default());
        assert!(!Place::default().option("--detach", "x").unwrap());
    }

    #[test]
    fn egress_policies_parse() {
        let e = |text: &str| {
            let mut into = None;
            egress_arg(&mut into, text).map(|()| into.unwrap().resolve(None))
        };
        assert_eq!(e("none"), Ok(Ok(Egress::None)));
        assert_eq!(e("full"), Ok(Ok(Egress::Full)));
        assert_eq!(
            e("allow:tcp:140.82.112.0/20:443,udp:10.0.0.1:53-54"),
            Ok(Ok(Egress::Rules(vec![
                Rule {
                    proto: Proto::Tcp,
                    addr: [140, 82, 112, 0],
                    prefix: 20,
                    lo: 443,
                    hi: 443
                },
                Rule {
                    proto: Proto::Udp,
                    addr: [10, 0, 0, 1],
                    prefix: 32,
                    lo: 53,
                    hi: 54
                },
            ])))
        );
        // `model` needs the model endpoint.
        assert!(e("model").unwrap().is_err());
        for bad in [
            "tcp:1.2.3.4",
            "tcp:1.2.3.4/33:80",
            "sctp:1.2.3.4:80",
            "tcp:1.2.3:80",
            "tcp:1.2.3.4:90-80",
            "tcp:1.2.3.4:70000",
        ] {
            assert!(e(bad).is_err(), "{bad}");
        }
        assert_eq!(
            show_egress(
                &e("tcp:140.82.112.0/20:443,any:10.0.0.0/8:1-1024")
                    .unwrap()
                    .unwrap()
            ),
            "tcp:140.82.112.0/20:443,any:10.0.0.0/8:1-1024"
        );
    }

    #[test]
    fn sizes_times_and_sets_parse() {
        assert_eq!(size("256M"), Ok(256 << 20));
        assert_eq!(size("2g"), Ok(2 << 30));
        assert_eq!(size("4096"), Ok(4096));
        assert!(size("2T").is_err() && size("x").is_err() && size("99999999999G").is_err());
        assert_eq!(time("500ms"), Ok(500_000_000));
        assert_eq!(time("2m"), Ok(120_000_000_000));
        assert!(time("5").is_err());
        assert_eq!(cpu_set("0,2-3"), Ok(0b1101));
        assert!(cpu_set("64").is_err() && cpu_set("3-2").is_err());
        assert_eq!(cpu_max("0.5"), Ok(500));
        assert!(cpu_max("0").is_err() && cpu_max("-1").is_err());
        assert!(place("--cpu-weight 0").is_err());
        assert!(place("--on-parent-end orphan").is_err());
    }

    /// The spec's small tree (`control-plane`, tree under control): two
    /// helpers indented under their agent with their egress and limits.
    #[test]
    fn ps_tree_indents_children_under_their_parent() {
        let cell = |id, parent, label: &str, egress, limits| CellInfo {
            id,
            image: b"idle".to_vec(),
            state: State::Running,
            tree: Some(TreeInfo {
                parent,
                root: 1,
                label: label.as_bytes().to_vec(),
                limits,
                egress,
            }),
        };
        let model = Egress::Rules(vec![endpoint_rule("10.0.2.2:8080").unwrap()]);
        let small = Limits {
            memory: 64 << 20,
            sockets: 4,
            cells: 0,
            ..Limits::NONE
        };
        let cells = [
            cell(
                1,
                0,
                "agent",
                model,
                Limits {
                    memory: 2 << 30,
                    cells: 4,
                    ..Limits::NONE
                },
            ),
            cell(5, 0, "", Egress::None, Limits::NONE),
            cell(2, 1, "tools", Egress::None, small),
            cell(3, 1, "search", Egress::None, small),
            CellInfo {
                id: 4,
                image: b"loaded".to_vec(),
                state: State::Suspended { bytes: 4096 },
                tree: None,
            },
        ];
        let rows = tree_rows(&cells, |_| "running".into());
        let ids: Vec<&str> = rows[1..]
            .iter()
            .map(|r| r.split("  ").next().unwrap())
            .collect();
        assert!(rows[1].starts_with("1 "), "{rows:?}");
        assert!(rows[1].contains("tcp:10.0.2.2/32:8080") && rows[1].contains("mem 2G, 4 cells"));
        assert!(
            rows[2].starts_with("  2 ") && rows[2].contains("tools"),
            "{rows:?}"
        );
        assert!(rows[2].contains("none") && rows[2].contains("mem 64M, 4 sockets, 0 cells"));
        assert!(
            rows[3].starts_with("  3 ") && rows[3].contains("search"),
            "{rows:?}"
        );
        assert!(
            rows[4].starts_with("5 ") && rows[4].contains("unlimited"),
            "{rows:?}"
        );
        assert!(
            rows[5].starts_with("4 ") && rows[5].contains("loaded"),
            "{rows:?}"
        );
        assert_eq!(ids.len(), 5);
    }
}
