//! The host side of a guest's network (design D6 of `add-virtio-net`, D5 of
//! `add-virtio-pci`): a TCP echo server the kernel's tests and benchmark
//! reach, QEMU's user network, and a tap device each for Firecracker and
//! Cloud Hypervisor when `sudo` allows it.

use crate::vmm::Vmm;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;

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
