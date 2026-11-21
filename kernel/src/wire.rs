//! The network card (spec `networking`, design D1, D4 and D5 of
//! `add-virtio-net`, D3 of `add-virtio-pci`): found among the virtio-mmio
//! transports the platform lists, else on PCI bus 0, given the address the
//! command line asks for, and polled by the cell host. Other virtio devices
//! are found the same way ([`find`]).

use crate::arch::console::BootConsole;
use crate::arch::platform::{self, Mmio};
use crate::memory::{self, DirectMap};
use crate::sync::Global;
use alloc::format;
use alloc::string::String;
use core::fmt::Write;
use rhizo_core::options::{BootOptions, IpOption};
use rhizo_hal::{ConsoleWriter, NetLink};
use rhizo_net::{Net, WireConfig};
use rhizo_virtio::pci::{self, Window};
use rhizo_virtio::{NET_DEVICE, ProbeError, VirtioNet};

type Card = VirtioNet<Mmio, DirectMap>;

static LINK: Global<Option<Card>> = Global::new(None);

/// Where unassigned PCI BARs get their addresses, once read from the
/// platform: shared by every device found, so that none overlap.
static WINDOW: Global<Option<Option<Window>>> = Global::new(None);

/// Takes frames, one at a time, for a device's rings and buffers.
type Alloc<'a> = &'a mut dyn FnMut() -> Option<u64>;

/// The command line's boot options.
pub fn options() -> BootOptions {
    let mut buf = [0u8; 1024];
    BootOptions::parse(platform::cmdline(&mut buf))
}

/// Brings up the first virtio network card, if any, and attaches it to
/// `net` with the address the command line gives (none: no address).
pub fn init(net: &mut Net, now_ns: u64, seed: u64) {
    let Some((at, mut card)) = find(
        NET_DEVICE,
        "net",
        rhizo_virtio::FRAMES,
        |regs, alloc| VirtioNet::probe(regs, DirectMap, alloc),
        |transport, alloc| VirtioNet::probe_pci(transport, DirectMap, alloc),
    ) else {
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
    LINK.with(|l| *l = Some(card));
}

/// The first virtio device of type `device` among the virtio-mmio
/// transports the platform lists, else on PCI bus 0, brought up by `mmio`
/// or `pci` with frames from one run of `frames`; and where it is, for
/// people. Failures are reported on the console, after `what`.
pub fn find<T>(
    device: u32,
    what: &str,
    frames: usize,
    mmio: impl Fn(Mmio, Alloc) -> Result<T, ProbeError>,
    pci: impl Fn(pci::Transport<Mmio>, Alloc) -> Result<T, ProbeError>,
) -> Option<(String, T)> {
    let mut found = None;
    let mut run: Option<(u64, usize)> = None;
    let alloc = &mut || {
        if run.is_none() {
            let start = memory::with_frames(|f| f.alloc_contiguous(frames)).flatten()?;
            run = Some((start, 0));
        }
        let (start, used) = run.as_mut()?;
        if *used == frames {
            return None;
        }
        *used += 1;
        Some(*start + (*used as u64 - 1) * rhizo_mm::FRAME_SIZE)
    };
    platform::for_each_virtio_mmio(|base| {
        if found.is_some() {
            return;
        }
        match mmio(Mmio::new(base), alloc) {
            Ok(d) => found = Some((format!("virtio-mmio {base:#x}"), d)),
            Err(ProbeError::Other(_)) => {}
            Err(e) => {
                let _ = writeln!(
                    ConsoleWriter(&mut BootConsole),
                    "{what}: virtio-mmio at {base:#x}: {e:?}"
                );
            }
        }
    });
    found.or_else(|| pci_device(device, what, pci, alloc))
}

/// The first virtio device of type `device` on PCI bus 0 (function 0 of
/// each device), brought up through the modern virtio-pci transport.
fn pci_device<T>(
    kind: u32,
    what: &str,
    probe: impl Fn(pci::Transport<Mmio>, Alloc) -> Result<T, ProbeError>,
    alloc: Alloc,
) -> Option<(String, T)> {
    let ecam = platform::pci_ecam()?;
    let mut window = WINDOW.with(|w| *w.get_or_insert_with(platform::pci_window));
    let report = |e: &dyn core::fmt::Debug, device: u64| {
        let _ = writeln!(
            ConsoleWriter(&mut BootConsole),
            "{what}: virtio-pci at 00:{device:02x}.0: {e:?}"
        );
    };
    for device in 0..32u64 {
        let config = ecam + (device << 15);
        if let Err(e) = platform::map_device(config, pci::FUNCTION_BYTES) {
            report(&e, device);
            return None;
        }
        let regions = match pci::regions(&mut Mmio::new(config), kind, window.as_mut()) {
            Ok(Some(regions)) => regions,
            Ok(None) => continue,
            Err(e) => {
                report(&e, device);
                continue;
            }
        };
        WINDOW.with(|w| *w = Some(window));
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
        match probe(transport, alloc) {
            Ok(d) => return Some((format!("virtio-pci 00:{device:02x}.0"), d)),
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
