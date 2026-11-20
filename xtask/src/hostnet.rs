//! The host side of a guest's network (design D6 of `add-virtio-net`): a
//! TCP echo server the kernel's tests and benchmark reach, QEMU's user
//! network, and a tap device for Firecracker when `sudo` allows it.

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
/// Where QEMU's user network shows the host's loopback to the guest.
const SLIRP_HOST: &str = "10.0.2.2";

/// How a guest on a VMM reaches the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Network {
    /// QEMU user network (slirp): DHCP, host at 10.0.2.2.
    Qemu { cmdline: String },
    /// Firecracker on a tap device with static addresses.
    Firecracker { tap: &'static str, cmdline: String },
    /// No network card: why.
    None(String),
}

/// The network a Rhizo guest gets on `vmm`, with a host echo server.
pub fn for_vmm(vmm: Vmm) -> Network {
    match vmm {
        Vmm::QemuMicrovm | Vmm::QemuVirt => match echo_server("127.0.0.1") {
            Ok(port) => Network::Qemu {
                cmdline: format!(" ip=dhcp echo={SLIRP_HOST}:{port}"),
            },
            Err(e) => Network::None(e),
        },
        Vmm::Firecracker => match tap().and_then(|()| echo_server(TAP_HOST)) {
            Ok(port) => Network::Firecracker {
                tap: TAP,
                cmdline: format!(" ip={TAP_GUEST}/24,{TAP_HOST} echo={TAP_HOST}:{port}"),
            },
            Err(e) => Network::None(e),
        },
        Vmm::QemuQ35 | Vmm::CloudHypervisor => Network::None(
            "this VMM attaches virtio devices only through PCI, and virtio-pci is not \
             implemented (design D1 of add-virtio-net)"
                .into(),
        ),
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

/// Creates the tap device for Firecracker once (`sudo -n`, no password).
fn tap() -> Result<(), String> {
    static TAP_READY: OnceLock<Result<(), String>> = OnceLock::new();
    TAP_READY
        .get_or_init(|| {
            let user = Command::new("id")
                .arg("-un")
                .output()
                .map_err(|e| e.to_string())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())?;
            let exists = Command::new("ip")
                .args(["link", "show", TAP])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            let mut steps: Vec<Vec<&str>> = Vec::new();
            if !exists {
                steps.push(vec![
                    "ip", "tuntap", "add", "dev", TAP, "mode", "tap", "user", &user,
                ]);
                steps.push(vec!["ip", "addr", "add", "172.16.0.1/24", "dev", TAP]);
            }
            steps.push(vec!["ip", "link", "set", TAP, "up"]);
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
                        "no tap device for Firecracker: `sudo -n {}` failed (needs \
                         password-less sudo)",
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
    fn pci_only_vmms_have_no_card() {
        for vmm in [Vmm::QemuQ35, Vmm::CloudHypervisor] {
            assert!(matches!(for_vmm(vmm), Network::None(why) if why.contains("virtio-pci")));
        }
        match for_vmm(Vmm::QemuMicrovm) {
            Network::Qemu { cmdline } => {
                assert!(cmdline.starts_with(" ip=dhcp echo=10.0.2.2:"), "{cmdline}");
            }
            other => panic!("{other:?}"),
        }
    }
}
