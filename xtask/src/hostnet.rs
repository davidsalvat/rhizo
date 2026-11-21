//! The host side of a guest's network (design D6 of `add-virtio-net`, D5 of
//! `add-virtio-pci`): a TCP echo server the kernel's tests and benchmark
//! reach, QEMU's user network, and a tap device each for Firecracker and
//! Cloud Hypervisor when `sudo` allows it; and a fake model server, an
//! HTTP chat API (design D1 of `add-reference-workloads` and of
//! `add-agent-workload`).

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
        Vmm::QemuMicrovm | Vmm::QemuVirt | Vmm::QemuQ35 => {
            match echo_server("127.0.0.1").and_then(|e| Ok((e, llm_server("127.0.0.1", 0)?))) {
                Ok((echo, llm)) => Network::Qemu {
                    cmdline: format!(" ip=dhcp echo={SLIRP_HOST}:{echo} llm={SLIRP_HOST}:{llm}"),
                },
                Err(e) => Network::None(e),
            }
        }
        Vmm::Firecracker => tap_network(TAP, TAP_HOST, TAP_GUEST),
        Vmm::CloudHypervisor => tap_network(CH_TAP, CH_TAP_HOST, CH_TAP_GUEST),
    }
}

/// Tap device `tap` with the host at `host`/24, the guest at `guest`, and
/// an echo server and a model server on the host's address.
fn tap_network(tap: &'static str, host: &str, guest: &str) -> Network {
    let servers = || Ok((echo_server(host)?, llm_server(host, 0)?));
    match tap_device(tap, host).and_then(|()| servers()) {
        Ok((echo, llm)) => Network::Tap {
            tap,
            cmdline: format!(" ip={guest}/24,{host} echo={host}:{echo} llm={host}:{llm}"),
        },
        Err(e) => Network::None(e),
    }
}

/// What a Linux guest in Firecracker needs to reach the model server over
/// the Firecracker tap device: the tap, and `NAME=VALUE` words for its
/// command line, which Linux hands to `/init` as its environment
/// (`rhizo_ip`, `rhizo_llm`).
pub fn linux_guest_network() -> Result<(&'static str, String), String> {
    tap_device(TAP, TAP_HOST)?;
    let port = llm_server(TAP_HOST, 0)?;
    Ok((
        TAP,
        format!(" rhizo_ip={TAP_GUEST} rhizo_llm={TAP_HOST}:{port}"),
    ))
}

/// The host's address as a guest on `vmm` sees it: QEMU's user network
/// shows the host's loopback at 10.0.2.2, the tap devices give the host an
/// address of its own.
pub fn host_address(vmm: Vmm) -> &'static str {
    match vmm {
        Vmm::QemuMicrovm | Vmm::QemuVirt | Vmm::QemuQ35 => SLIRP_HOST,
        Vmm::Firecracker => TAP_HOST,
        Vmm::CloudHypervisor => CH_TAP_HOST,
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
pub fn llm_server(addr: &str, delay_ms: u32) -> Result<u16, String> {
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
                    thread::spawn(move || serve_model(stream, delay));
                }
            });
            Ok(port)
        })
        .clone()
}

/// Answers HTTP requests on one connection, kept alive until the client
/// closes it or asks for `Connection: close`.
fn serve_model(mut stream: TcpStream, delay: Duration) {
    let _ = stream.set_nodelay(true);
    let mut pending = Vec::new();
    let mut buf = [0u8; 16384];
    let mut served = 0;
    loop {
        let Some((end, close)) = request_end(&pending) else {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => pending.extend_from_slice(&buf[..n]),
            }
            continue;
        };
        let request: Vec<u8> = pending.drain(..end).collect();
        served += 1;
        thread::sleep(delay);
        let body = model_answer(&request, served);
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: {}\r\n\r\n{body}",
            body.len(),
            if close { "close" } else { "keep-alive" }
        );
        if stream.write_all(reply.as_bytes()).is_err() || close {
            return;
        }
    }
}

/// Where the first complete HTTP request of `bytes` ends (headers, then
/// `Content-Length` bytes of body), and whether it asks to close the
/// connection; `None` until it is complete.
fn request_end(bytes: &[u8]) -> Option<(usize, bool)> {
    let head = bytes.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let text = String::from_utf8_lossy(&bytes[..head]);
    let header = |name: &str| {
        text.lines().find_map(|l| {
            let (n, v) = l.split_once(':')?;
            n.trim()
                .eq_ignore_ascii_case(name)
                .then(|| v.trim().to_string())
        })
    };
    let length: usize = header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let close = header("connection").is_some_and(|v| v.eq_ignore_ascii_case("close"));
    (bytes.len() >= head + length).then_some((head + length, close))
}

/// The fake model's chat completion for `request` (the `served`-th on its
/// connection): always a call to the agent's `compute` tool, with arguments
/// derived from the whole request body (FNV-1a), so the same conversation
/// always gets the same answer. Built without parsing JSON: the server's
/// own time is part of every turn the benchmark measures, on both sides.
fn model_answer(request: &[u8], served: u32) -> String {
    let head = request
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map_or(0, |p| p + 4);
    let hash = request[head..]
        .iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, &b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        });
    if contains(&request[head..], b"\"run_python\"") {
        return chat_answer(&request[head..], hash, served);
    }
    let (n, seed, id) = (500 + hash % 500, (hash >> 20) % 1000, hash as u32);
    format!(
        "{{\"object\": \"chat.completion\", \"model\": \"rhizo-fake\", \"choices\": [{{\
         \"index\": 0, \"finish_reason\": \"tool_calls\", \"message\": {{\"role\": \"assistant\", \
         \"content\": null, \"tool_calls\": [{{\"id\": \"call_{id:08x}\", \"type\": \"function\", \
         \"function\": {{\"name\": \"compute\", \
         \"arguments\": \"{{\\\"n\\\": {n}, \\\"seed\\\": {seed}}}\"}}}}]}}}}], \
         \"x_requests_on_connection\": {served}}}"
    )
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// The fake model as the chat agent sees it (`cells/python/agent_chat.py`,
/// change `add-interactive-chat`): after a user's message, a call to its
/// `run_python` tool with a snippet derived from the conversation's hash;
/// after the tool's result, an answer quoting what the snippet printed.
/// Only the last message's role and content are looked for, as Python's
/// `json` writes them.
fn chat_answer(body: &[u8], hash: u64, served: u32) -> String {
    let text = String::from_utf8_lossy(body);
    let last = text
        .rfind(r#""role": ""#)
        .map(|at| &text[at + r#""role": ""#.len()..]);
    let message = match last {
        Some(rest) if rest.starts_with(r#"tool""#) => {
            // The tool's output, still escaped as a JSON string, which it
            // stays inside the answer.
            let printed = rest
                .split_once(r#""content": ""#)
                .map_or("", |(_, c)| json_string(c))
                .trim_end_matches(r"\n");
            format!(r#""content": "The snippet printed {printed}.""#)
        }
        _ => format!(
            r#""content": null, "tool_calls": [{{"id": "call_{:08x}", "type": "function", "function": {{"name": "run_python", "arguments": "{{\"code\": \"print(sum(i * i for i in range({})))\"}}"}}}}]"#,
            hash as u32,
            100 + hash % 900
        ),
    };
    format!(
        r#"{{"object": "chat.completion", "model": "rhizo-fake", "choices": [{{"index": 0, "message": {{"role": "assistant", {message}}}}}], "x_requests_on_connection": {served}}}"#
    )
}

/// The body of the JSON string literal `rest` starts inside, escapes kept:
/// up to its closing quote.
fn json_string(rest: &str) -> &str {
    let mut escaped = false;
    for (i, c) in rest.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => return &rest[..i],
            _ => {}
        }
    }
    rest
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

    /// Sends one chat request on `s` and returns the answer's JSON body.
    fn chat(s: &mut TcpStream, body: &str, close: bool) -> serde_json::Value {
        let connection = if close { "Connection: close\r\n" } else { "" };
        let request = format!(
            "POST /v1/chat/completions HTTP/1.1\r\nHost: model\r\n{connection}\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        s.write_all(request.as_bytes()).unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = s.read(&mut buf).unwrap();
            assert!(n > 0, "the server closed early");
            got.extend_from_slice(&buf[..n]);
            if let Some((end, _)) = request_end(&got) {
                assert_eq!(end, got.len(), "one answer");
                break;
            }
        }
        let text = String::from_utf8(got).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{text}");
        let (_, body) = text.split_once("\r\n\r\n").unwrap();
        serde_json::from_str(body).unwrap()
    }

    #[test]
    fn the_model_answers_on_a_kept_alive_connection_deterministically() {
        let port = llm_server("127.0.0.1", 0).unwrap();
        assert_eq!(llm_server("127.0.0.1", 0), Ok(port), "started once");
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let a = chat(
            &mut s,
            r#"{"messages": [{"role": "user", "content": "hi"}]}"#,
            false,
        );
        // Same conversation, same answer; another, another.
        let b = chat(
            &mut s,
            r#"{"messages": [{"role": "user", "content": "hi"}]}"#,
            false,
        );
        let c = chat(
            &mut s,
            r#"{"messages":[{"content":"ho","role":"user"}]}"#,
            true,
        );
        assert_eq!(a["x_requests_on_connection"], 1);
        assert_eq!(b["x_requests_on_connection"], 2, "the connection was kept");
        let call = |v: &serde_json::Value| v["choices"][0]["message"]["tool_calls"][0].clone();
        assert_eq!(call(&a), call(&b));
        assert_ne!(call(&a), call(&c));
        assert_eq!(call(&a)["function"]["name"], "compute");
        let args: serde_json::Value =
            serde_json::from_str(call(&a)["function"]["arguments"].as_str().unwrap()).unwrap();
        assert!((500..1000).contains(&args["n"].as_u64().unwrap()), "{args}");
        let mut rest = Vec::new();
        s.read_to_end(&mut rest).unwrap();
        assert!(rest.is_empty(), "closed after `Connection: close`");
    }

    #[test]
    fn the_chat_model_calls_the_snippet_tool_then_quotes_it() {
        let port = llm_server("127.0.0.1", 0).unwrap();
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let tools = r#""tools": [{"function": {"name": "run_python"}}]"#;
        let asked = format!(r#"{{"messages": [{{"role": "user", "content": "hi"}}], {tools}}}"#);
        let a = chat(&mut s, &asked, false);
        let call = &a["choices"][0]["message"]["tool_calls"][0];
        assert_eq!(call["function"]["name"], "run_python");
        let args: serde_json::Value =
            serde_json::from_str(call["function"]["arguments"].as_str().unwrap()).unwrap();
        let code = args["code"].as_str().unwrap();
        assert!(
            code.starts_with("print(sum(i * i for i in range("),
            "{code}"
        );
        let again = chat(&mut s, &asked, false);
        assert_eq!(again["choices"], a["choices"], "deterministic");
        let answered = format!(
            r#"{{"messages": [{{"role": "user", "content": "hi"}}, {{"role": "tool", "tool_call_id": "c", "content": "a \"quoted\" 42\n"}}], {tools}}}"#
        );
        let b = chat(&mut s, &answered, true);
        assert_eq!(
            b["choices"][0]["message"]["content"],
            r#"The snippet printed a "quoted" 42."#
        );
        assert!(b["choices"][0]["message"]["tool_calls"].is_null());
    }

    #[test]
    fn requests_end_after_their_body() {
        assert_eq!(request_end(b"GET / HTTP/1.1\r\n"), None);
        assert_eq!(request_end(b"GET / HTTP/1.1\r\n\r\nGET"), Some((18, false)));
        let post = b"POST / HTTP/1.1\r\ncontent-length: 3\r\nConnection: close\r\n\r\nab";
        assert_eq!(request_end(post), None, "body not complete");
        let mut whole = post.to_vec();
        whole.push(b'c');
        assert_eq!(request_end(&whole), Some((whole.len(), true)));
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
                    assert!(cmdline.contains(" llm=10.0.2.2:"), "{cmdline}");
                }
                other => panic!("{other:?}"),
            }
        }
    }
}
