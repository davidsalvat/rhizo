//! The host's deny floor (design D7 of `add-cell-hierarchy`): what no
//! egress policy reaches. The kernel always holds the host's own addresses
//! as the guest sees them in it; `rhizo up` adds `--deny` rules and carves
//! out the model endpoint (`--model`) and `--allow-host-port` exceptions,
//! each an exact TCP endpoint. A client giving a cell `--egress model` for
//! an endpoint not carved out yet carves out that endpoint too: the host
//! names it on its command line, and the kernel audits every change.
//!
//! The server records the floor in the state file, and a client changes it
//! there and in the kernel together, under a lock, so that two clients
//! carving at once keep both endpoints.

use crate::client::Client;
use crate::state::{Dir, Info};
use crate::tree::{endpoint_rule, parse_rule};
use rhizo_control::{Proto, Request, Rule};
use xtask::vmm::Vmm;

/// Most rules in each part of the floor (`rhizo_cap::limits::FLOOR_MAX`).
pub const MAX: usize = 64;

/// The floor options of `rhizo up`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// `--model HOST:PORT` (or a URL naming them).
    pub model: Option<String>,
    /// `--deny CIDR`.
    pub deny: Vec<String>,
    /// `--allow-host-port PORT`.
    pub allow_host_ports: Vec<u16>,
}

impl Options {
    /// Takes `flag`'s `value` if it is a floor option; `Ok(false)` if not.
    pub fn take(&mut self, flag: &str, value: &str) -> Result<bool, String> {
        match flag {
            "--model" => self.model = Some(value.into()),
            "--deny" => {
                deny_rule(value)?;
                self.deny.push(value.into());
            }
            "--allow-host-port" => self.allow_host_ports.push(
                value
                    .parse()
                    .ok()
                    .filter(|&p| p != 0)
                    .ok_or_else(|| format!("--allow-host-port {value}: a port"))?,
            ),
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The options again, for the server `rhizo up` starts.
    pub fn args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(model) = &self.model {
            args.extend(["--model".into(), model.clone()]);
        }
        for cidr in &self.deny {
            args.extend(["--deny".into(), cidr.clone()]);
        }
        for port in &self.allow_host_ports {
            args.extend(["--allow-host-port".into(), port.to_string()]);
        }
        args
    }

    /// The floor these options give on `vmm`.
    pub fn floor(&self, vmm: Vmm) -> Result<Floor, String> {
        let deny = self
            .deny
            .iter()
            .map(|c| deny_rule(c))
            .collect::<Result<Vec<_>, _>>()?;
        let model = self
            .model
            .as_deref()
            .map(|m| crate::agent::guest_endpoint(m, vmm).and_then(|e| endpoint_rule(&e)))
            .transpose()?;
        let host = xtask::hostnet::host_address(vmm);
        let mut carved: Vec<Rule> = model.into_iter().collect();
        for port in &self.allow_host_ports {
            carved.push(endpoint_rule(&format!("{host}:{port}"))?);
        }
        if deny.len() > MAX || carved.len() > MAX {
            return Err(format!(
                "the deny floor holds at most {MAX} rules of each kind"
            ));
        }
        Ok(Floor {
            deny,
            carved,
            model,
        })
    }
}

/// `--deny CIDR`: every protocol and port of `A.B.C.D[/N]`.
fn deny_rule(cidr: &str) -> Result<Rule, String> {
    parse_rule(&format!("any:{cidr}:0-65535")).map_err(|_| format!("--deny {cidr}: A.B.C.D/N"))
}

/// The floor the kernel holds, and the model endpoint `rhizo up` named.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Floor {
    pub deny: Vec<Rule>,
    pub carved: Vec<Rule>,
    pub model: Option<Rule>,
}

impl Floor {
    /// The request that gives it to the kernel.
    pub fn request(&self) -> Request {
        Request::Floor {
            deny: self.deny.clone(),
            carved: self.carved.clone(),
        }
    }

    /// Its lines in the state file.
    pub fn encode(&self) -> String {
        let list = |rules: &[Rule]| rules.iter().map(text).collect::<Vec<_>>().join(",");
        let mut out = format!(
            "floor_deny={}\nfloor_carved={}\n",
            list(&self.deny),
            list(&self.carved)
        );
        if let Some(m) = &self.model {
            out += &format!("model={}\n", text(m));
        }
        out
    }

    /// The floor in a state file's `text`, if it has one.
    pub fn decode(text: &str) -> Option<Self> {
        let field = |name: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(name)?.strip_prefix('='))
        };
        let list = |v: &str| {
            v.split(',')
                .filter(|s| !s.is_empty())
                .map(|r| parse_rule(r).ok())
                .collect::<Option<Vec<_>>>()
        };
        Some(Floor {
            deny: list(field("floor_deny")?)?,
            carved: list(field("floor_carved")?)?,
            model: match field("model") {
                Some(m) => Some(parse_rule(m).ok()?),
                None => None,
            },
        })
    }
}

/// A rule as `rhizo` takes it: `tcp:10.0.2.2/32:11434-11434`.
pub fn text(r: &Rule) -> String {
    let proto = match r.proto {
        Proto::Tcp => "tcp",
        Proto::Udp => "udp",
        Proto::Any => "any",
    };
    let [a, b, c, d] = r.addr;
    format!("{proto}:{a}.{b}.{c}.{d}/{}:{}-{}", r.prefix, r.lo, r.hi)
}

/// Carves `model`, the endpoint a cell is given `--egress model` for, out
/// of the floor of `client`'s VM if it is not yet: in the kernel, then in
/// the state file. Nothing for a kernel of protocol 3, which has no floor.
pub fn carve(client: &mut Client, dir: &Dir, model: Rule) -> Result<(), String> {
    if !client.current() {
        return Ok(());
    }
    let lock = std::fs::File::create(dir.floor_lock())
        .map_err(|e| format!("cannot lock the floor: {e}"))?;
    lock.lock()
        .map_err(|e| format!("cannot lock the floor: {e}"))?;
    let mut info = Info::read(dir).ok_or("the VM is down")?;
    let mut floor = info.floor.clone().unwrap_or_default();
    if floor.carved.contains(&model) {
        return Ok(());
    }
    if floor.carved.len() == MAX {
        return Err(format!("the deny floor already carves out {MAX} endpoints"));
    }
    floor.carved.push(model);
    client.call(&floor.request())?;
    info.floor = Some(floor);
    info.write(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(words: &str) -> Result<Options, String> {
        let mut o = Options::default();
        let w: Vec<&str> = words.split_whitespace().collect();
        for pair in w.chunks(2) {
            assert!(o.take(pair[0], pair[1])?, "{}", pair[0]);
        }
        Ok(o)
    }

    #[test]
    fn up_options_give_the_floor() {
        let o = options(
            "--model http://localhost:11434 --deny 192.168.0.0/16 --deny 10.9.9.9 \
             --allow-host-port 5432",
        )
        .unwrap();
        let floor = o.floor(Vmm::QemuMicrovm).unwrap();
        let rule = |t: &str| parse_rule(t).unwrap();
        assert_eq!(
            floor.deny,
            [
                rule("any:192.168.0.0/16:0-65535"),
                rule("any:10.9.9.9/32:0-65535")
            ]
        );
        // On QEMU's user network the host is its gateway.
        let host = xtask::hostnet::host_address(Vmm::QemuMicrovm);
        let model = rule(&format!("tcp:{host}:11434"));
        assert_eq!(floor.carved, [model, rule(&format!("tcp:{host}:5432"))]);
        assert_eq!(floor.model, Some(model));
        // The server gets the same options.
        let again = options(&o.args().join(" ")).unwrap();
        assert_eq!(again, o);
        assert!(options("--deny 300.0.0.0/8").is_err());
        assert!(options("--allow-host-port 0").is_err());
        assert!(options("--allow-host-port http").is_err());
        let mut o = Options::default();
        assert_eq!(o.take("--memory", "1"), Ok(false));
        let many = (0..=MAX)
            .map(|i| format!("--deny 10.{i}.0.0/16"))
            .collect::<Vec<_>>();
        assert!(
            options(&many.join(" "))
                .unwrap()
                .floor(Vmm::QemuMicrovm)
                .is_err()
        );
    }

    #[test]
    fn the_floor_round_trips_in_the_state_file() {
        let floor = options("--model 10.0.2.2:8080 --deny 172.16.0.0/12 --allow-host-port 22")
            .unwrap()
            .floor(Vmm::QemuMicrovm)
            .unwrap();
        assert_eq!(Floor::decode(&floor.encode()), Some(floor));
        let empty = Floor::default();
        assert_eq!(Floor::decode(&empty.encode()), Some(empty));
        assert_eq!(Floor::decode("pid=1\n"), None, "a server that sent none");
        assert_eq!(Floor::decode("floor_deny=bad\nfloor_carved=\n"), None);
    }
}
