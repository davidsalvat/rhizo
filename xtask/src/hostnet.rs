//! The host side of a guest's network (design D6 of `add-virtio-net`, D5 of
//! `add-virtio-pci`): a TCP echo server the kernel's tests and benchmark
//! reach, QEMU's user network, and a tap device each for Firecracker and
//! Cloud Hypervisor when `sudo` allows it; and, for the laptop workload, a
//! fake model server (design D1 of `add-reference-workloads`).

use crate::vmm::Vmm;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

/// Tap device given to Firecracker, its host address and the guest's.
const TAP: &str = "rhizotap0";
const TAP_HOST: &str = "172.16.0.1";
const TAP_GUEST: &str = "172.16.0.2";
/// The same for Cloud Hypervisor, on a subnet of its own.
const CH_TAP: &str = "rhizotap1";
const CH_TAP_HOST: &str = "172.16.1.1";
const CH_TAP_GUEST: &str = "172.16.1.2";
/// Where QEMU's user network shows the host's loopback to the guest.
const SLIRP_HOST: &str = "10.0.2.2";

/// How a guest on a VMM reaches the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Network {
    /// QEMU user network (slirp): DHCP, host at 10.0.2.2.
    Qemu { cmdline: String },
    /// A tap device with static addresses (Firecracker, Cloud Hypervisor).
    Tap { tap: &'static str, cmdline: String },
    /// No network card: why.
    None(String),
}

/// The network a Rhizo guest gets on `vmm`, with a host echo server.
pub fn for_vmm(vmm: Vmm) -> Network {
    match vmm {
        Vmm::QemuMicrovm | Vmm::QemuVirt | Vmm::QemuQ35 => match echo_server("127.0.0.1") {
            Ok(port) => Network::Qemu {
                cmdline: format!(" ip=dhcp echo={SLIRP_HOST}:{port}"),
            },
            Err(e) => Network::None(e),
        },
        Vmm::Firecracker => tap_network(TAP, TAP_HOST, TAP_GUEST),
        Vmm::CloudHypervisor => tap_network(CH_TAP, CH_TAP_HOST, CH_TAP_GUEST),
    }
}

/// Tap device `tap` with the host at `host`/24, the guest at `guest`, and
/// an echo server on the host's address.
fn tap_network(tap: &'static str, host: &str, guest: &str) -> Network {
    match tap_device(tap, host).and_then(|()| echo_server(host)) {
        Ok(port) => Network::Tap {
            tap,
            cmdline: format!(" ip={guest}/24,{host} echo={host}:{port}"),
        },
        Err(e) => Network::None(e),
    }
}

/// The port of a TCP echo server listening on `addr` (started once).
fn echo_server(addr: &str) -> Result<u16, String> {
    static SERVERS: OnceLock<Mutex<BTreeMap<String, Result<u16, String>>>> = OnceLock::new();
    let servers = SERVERS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut servers = servers.lock().map_err(|e| e.to_string())?;
    servers
        .entry(addr.to_string())
        .or_insert_with(|| {
            let listener = TcpListener::bind((addr, 0))
                .map_err(|e| format!("cannot start an echo server on {addr}: {e}"))?;
            let port = listener.local_addr().map_err(|e| e.to_string())?.port();
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    thread::spawn(move || echo(stream));
                }
            });
            Ok(port)
        })
        .clone()
}

/// The `llm=HOST:PORT` option for a guest on `vmm`: a fake model server on
/// the host's side of its network, answering after `delay_ms` (`None`
/// without a network).
pub fn llm_option(vmm: Vmm, delay_ms: u32) -> Option<String> {
    let (bind, seen_as) = match for_vmm(vmm) {
        Network::Qemu { .. } => ("127.0.0.1", SLIRP_HOST),
        Network::Tap { tap, .. } if tap == TAP => (TAP_HOST, TAP_HOST),
        Network::Tap { .. } => (CH_TAP_HOST, CH_TAP_HOST),
        Network::None(_) => return None,
    };
    let port = llm_server(bind, delay_ms).ok()?;
    Some(format!("llm={seen_as}:{port}"))
}

/// The port of a fake model server on `addr` answering after `delay_ms`
/// (started once per address and delay).
fn llm_server(addr: &str, delay_ms: u32) -> Result<u16, String> {
    type Servers = BTreeMap<(String, u32), Result<u16, String>>;
    static SERVERS: OnceLock<Mutex<Servers>> = OnceLock::new();
    let servers = SERVERS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut servers = servers.lock().map_err(|e| e.to_string())?;
    servers
        .entry((addr.to_string(), delay_ms))
        .or_insert_with(|| {
            let listener = TcpListener::bind((addr, 0))
                .map_err(|e| format!("cannot start a model server on {addr}: {e}"))?;
            let port = listener.local_addr().map_err(|e| e.to_string())?.port();
            let delay = Duration::from_millis(delay_ms.into());
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    thread::spawn(move || answer(stream, delay));
                }
            });
            Ok(port)
        })
        .clone()
}

/// What the fake model answers: a short completion naming a tool.
pub const LLM_ANSWER: &str = "{\"choices\":[{\"message\":{\"role\":\"assistant\",\
     \"content\":\"noted\",\"tool_calls\":[{\"function\":{\"name\":\"note\"}}]}}]}";

/// Reads one HTTP request (headers and `Content-Length` bytes of body),
/// waits `delay`, answers [`LLM_ANSWER`] and closes the connection.
fn answer(mut stream: TcpStream, delay: Duration) {
    let _ = stream.set_nodelay(true);
    let mut request = Vec::new();
    let mut buf = [0u8; 4096];
    let body_end = loop {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => request.extend_from_slice(&buf[..n]),
        }
        if let Some(end) = request_end(&request) {
            break end;
        }
    };
    if request.len() < body_end {
        return;
    }
    thread::sleep(delay);
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{LLM_ANSWER}",
        LLM_ANSWER.len()
    );
    let _ = stream.write_all(reply.as_bytes());
}

/// Where a complete HTTP request in `bytes` ends (headers, then
/// `Content-Length` bytes), once its headers are in.
fn request_end(bytes: &[u8]) -> Option<usize> {
    let head = bytes.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let length = String::from_utf8_lossy(&bytes[..head])
        .lines()
        .find_map(|l| {
            let (name, value) = l.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    let end = head + length;
    (bytes.len() >= end).then_some(end)
}

fn echo(mut stream: TcpStream) {
    let _ = stream.set_nodelay(true);
    let mut buf = [0u8; 4096];
    while let Ok(n) = stream.read(&mut buf) {
        if n == 0 || stream.write_all(&buf[..n]).is_err() {
            break;
        }
    }
}

/// Creates tap device `tap` with the host address `host`/24 once (`sudo
/// -n`, no password).
fn tap_device(tap: &'static str, host: &str) -> Result<(), String> {
    static READY: OnceLock<Mutex<BTreeMap<&'static str, Result<(), String>>>> = OnceLock::new();
    let ready = READY.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut ready = ready.lock().map_err(|e| e.to_string())?;
    ready
        .entry(tap)
        .or_insert_with(|| {
            let user = Command::new("id")
                .arg("-un")
                .output()
                .map_err(|e| e.to_string())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())?;
            let exists = Command::new("ip")
                .args(["link", "show", tap])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            let cidr = format!("{host}/24");
            let mut steps: Vec<Vec<&str>> = Vec::new();
            if !exists {
                steps.push(vec![
                    "ip", "tuntap", "add", "dev", tap, "mode", "tap", "user", &user,
                ]);
                steps.push(vec!["ip", "addr", "add", &cidr, "dev", tap]);
            }
            steps.push(vec!["ip", "link", "set", tap, "up"]);
            for step in steps {
                let ok = Command::new("sudo")
                    .arg("-n")
                    .args(&step)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success());
                if !ok {
                    return Err(format!(
                        "no tap device {tap}: `sudo -n {}` failed (needs password-less sudo)",
                        step.join(" ")
                    ));
                }
            }
            Ok(())
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_echo_server_echoes() {
        let port = echo_server("127.0.0.1").unwrap();
        assert_eq!(echo_server("127.0.0.1"), Ok(port), "started once");
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(b"ping").unwrap();
        let mut back = [0u8; 4];
        s.read_exact(&mut back).unwrap();
        assert_eq!(&back, b"ping");
    }

    #[test]
    fn the_model_server_answers_a_request() {
        let port = llm_server("127.0.0.1", 0).unwrap();
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(b"POST /v1/chat HTTP/1.1\r\nContent-Length: 2\r\n\r\n{}")
            .unwrap();
        let mut reply = String::new();
        s.read_to_string(&mut reply).unwrap();
        assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
        assert!(reply.ends_with(LLM_ANSWER), "{reply}");
        assert!(reply.contains(&format!("Content-Length: {}\r\n", LLM_ANSWER.len())));
    }

    #[test]
    fn requests_end_after_their_body() {
        assert_eq!(request_end(b"GET / HTTP/1.1\r\n"), None);
        assert_eq!(request_end(b"GET / HTTP/1.1\r\n\r\n"), Some(18));
        let post = b"POST / HTTP/1.1\r\ncontent-length: 3\r\n\r\nab";
        assert_eq!(request_end(post), None, "body not complete");
        let mut whole = post.to_vec();
        whole.push(b'c');
        assert_eq!(request_end(&whole), Some(whole.len()));
    }

    #[test]
    fn qemu_guests_reach_the_model_through_the_user_network() {
        let option = llm_option(Vmm::QemuMicrovm, 0).unwrap();
        assert!(option.starts_with("llm=10.0.2.2:"), "{option}");
    }

    #[test]
    fn qemu_machines_get_the_user_network() {
        for vmm in [Vmm::QemuMicrovm, Vmm::QemuQ35, Vmm::QemuVirt] {
            match for_vmm(vmm) {
                Network::Qemu { cmdline } => {
                    assert!(cmdline.starts_with(" ip=dhcp echo=10.0.2.2:"), "{cmdline}");
                }
                other => panic!("{other:?}"),
            }
        }
    }
}
