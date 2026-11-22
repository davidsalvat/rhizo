//! The network card (spec `networking`, design D1, D4 and D5 of
//! `add-virtio-net`, D3 of `add-virtio-pci`): found among the virtio-mmio
//! transports the platform lists, else on PCI bus 0, given the address the
//! command line asks for, and polled by the cell host.

use crate::arch::console::BootConsole;
use crate::arch::platform::{self, Mmio};
use crate::memory::{self, DirectMap};
use crate::sync::Global;
use alloc::format;
use alloc::string::String;
use core::fmt::Write;
use rhizo_core::options::{BootOptions, IpOption};
use rhizo_hal::{ConsoleWriter, NetLink};
use rhizo_net::{Egress, Net, Proto, Rule, WireConfig};
use rhizo_virtio::{NET_DEVICE, ProbeError, VirtioNet, pci};

type Card = VirtioNet<Mmio, DirectMap>;

static LINK: Global<Option<Card>> = Global::new(None);

/// The command line's boot options.
pub fn options() -> BootOptions {
    let mut buf = [0u8; 1024];
    BootOptions::parse(platform::cmdline(&mut buf))
}

/// Brings up the first virtio network card, if any, and attaches it to
/// `net` with the address the command line gives (none: no address).
pub fn init(net: &mut Net, now_ns: u64, seed: u64) {
    let mut card = None;
    // One run of frames for the card's rings and buffers.
    let mut run: Option<(u64, usize)> = None;
    let alloc = &mut || {
        if run.is_none() {
            let start =
                memory::with_frames(|f| f.alloc_contiguous(rhizo_virtio::FRAMES)).flatten()?;
            run = Some((start, 0));
        }
        let (start, used) = run.as_mut()?;
        if *used == rhizo_virtio::FRAMES {
            return None;
        }
        *used += 1;
        Some(*start + (*used as u64 - 1) * rhizo_mm::FRAME_SIZE)
    };
    platform::for_each_virtio_mmio(|base| {
        if card.is_some() {
            return;
        }
        match VirtioNet::probe(Mmio::new(base), DirectMap, alloc) {
            Ok(c) => card = Some((format!("virtio-mmio {base:#x}"), c)),
            Err(ProbeError::Other(_)) => {}
            Err(e) => {
                let _ = writeln!(
                    ConsoleWriter(&mut BootConsole),
                    "net: virtio-mmio at {base:#x}: {e:?}"
                );
            }
        }
    });
    if card.is_none() {
        card = pci_card(alloc);
    }
    let Some((at, mut card)) = card else {
        return;
    };
    let m = card.mac();
    let _ = writeln!(
        ConsoleWriter(&mut BootConsole),
        "net: card {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} at {at}",
        m[0],
        m[1],
        m[2],
        m[3],
        m[4],
        m[5]
    );
    let config = match options().ip {
        Some(IpOption::Dhcp) => Some(WireConfig::Dhcp),
        Some(IpOption::Static {
            addr,
            prefix,
            gateway,
        }) => Some(WireConfig::Static {
            addr,
            prefix,
            gateway,
        }),
        None => None,
    };
    if let Some(config) = config {
        net.attach(&mut card, config, now_ns, seed);
    }
    // The deny floor (design D7 of `add-cell-hierarchy`): the card's
    // gateway and name servers, the host as the guest sees it, are always
    // in it; carved out are the model endpoint and, for the kernels `xtask`
    // tests and measures, its echo server (`echo=`, given to no other
    // kernel), as `rhizo up --allow-host-port` would. At most two rules.
    let opts = options();
    let carved: alloc::vec::Vec<Rule> = [opts.llm, opts.echo]
        .into_iter()
        .flatten()
        .filter_map(|(ip, port)| Rule::new(Proto::Tcp, ip, 32, port, port))
        .collect();
    let _ = net.set_floor(&[], &carved);
    LINK.with(|l| *l = Some(card));
}

/// The egress policy `model` (design D7 of `add-cell-hierarchy`): the
/// host's model endpoint (`llm=` on the command line); `None` without one.
#[cfg_attr(
    not(any(feature = "ktest", feature = "bench-cell")),
    allow(dead_code, reason = "used by in-kernel tests and benchmarks")
)]
pub fn model_egress() -> Egress {
    let Some((ip, port)) = options().llm else {
        return Egress::None;
    };
    Rule::new(Proto::Tcp, ip, 32, port, port)
        .and_then(|r| Egress::rules(&[r]).ok())
        .unwrap_or_default()
}

/// A policy allowing TCP to the in-kernel tests' echo server only (`echo=`);
/// `None` without one.
#[cfg_attr(
    not(feature = "ktest"),
    allow(dead_code, reason = "used by in-kernel tests")
)]
pub fn echo_egress() -> Egress {
    let Some((ip, port)) = options().echo else {
        return Egress::None;
    };
    Rule::new(Proto::Tcp, ip, 32, port, port)
        .and_then(|r| Egress::rules(&[r]).ok())
        .unwrap_or_default()
}

/// The first virtio network card on PCI bus 0 (function 0 of each device),
/// brought up through the modern virtio-pci transport.
fn pci_card(alloc: &mut dyn FnMut() -> Option<u64>) -> Option<(String, Card)> {
    let ecam = platform::pci_ecam()?;
    let mut window = platform::pci_window();
    let report = |what: &dyn core::fmt::Debug, device: u64| {
        let _ = writeln!(
            ConsoleWriter(&mut BootConsole),
            "net: virtio-pci at 00:{device:02x}.0: {what:?}"
        );
    };
    for device in 0..32u64 {
        let config = ecam + (device << 15);
        if let Err(e) = platform::map_device(config, pci::FUNCTION_BYTES) {
            report(&e, device);
            return None;
        }
        let regions = match pci::regions(&mut Mmio::new(config), NET_DEVICE, window.as_mut()) {
            Ok(Some(regions)) => regions,
            Ok(None) => continue,
            Err(e) => {
                report(&e, device);
                continue;
            }
        };
        let regs = |(at, len)| platform::map_device(at, len).map(|()| Mmio::new(at));
        let transport = match (
            regs(regions.common),
            regs(regions.notify),
            regs(regions.device),
        ) {
            (Ok(c), Ok(n), Ok(d)) => pci::Transport::new(c, n, regions.notify_multiplier, d),
            (Err(e), ..) | (_, Err(e), _) | (.., Err(e)) => {
                report(&e, device);
                continue;
            }
        };
        match VirtioNet::probe_pci(transport, DirectMap, alloc) {
            Ok(card) => return Some((format!("virtio-pci 00:{device:02x}.0"), card)),
            Err(e) => report(&e, device),
        }
    }
    None
}

/// Longest time the card goes unpolled while cells use it and nothing else
/// runs (design D4).
const BUSY_NS: u64 = 20_000;

/// Polls the card and `smoltcp`; returns in how many nanoseconds they want
/// to be polled again, if there is a card.
pub fn poll(net: &mut Net, now_ns: u64) -> Option<u64> {
    LINK.with(|l| {
        let link = l.as_mut()?;
        let delay = net.poll_wire(link, now_ns).unwrap_or(u64::MAX);
        Some(if net.wire_busy() {
            delay.min(BUSY_NS)
        } else {
            delay
        })
    })
}
