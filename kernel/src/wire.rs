//! The network card (spec `networking`, design D1, D4 and D5 of
//! `add-virtio-net`, D3 of `add-virtio-pci`): found among the virtio-mmio
//! transports the platform lists, else on PCI bus 0, given the address the
//! command line asks for, and polled by the cell host. A virtio-mmio card
//! whose interrupt the platform can route also interrupts the boot CPU when
//! a frame arrives (design D1 and D2 of `add-virtio-interrupts`).

use crate::arch::console::BootConsole;
use crate::arch::platform::{self, Mmio};
use crate::memory::{self, DirectMap};
use crate::sync::Global;
use alloc::format;
use alloc::string::String;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, Ordering};
use rhizo_core::options::{BootOptions, IpOption};
use rhizo_hal::{ConsoleWriter, NetLink};
use rhizo_net::{Net, WireConfig};
use rhizo_virtio::{ProbeError, VirtioNet, pci};

type Card = VirtioNet<Mmio, DirectMap>;

static LINK: Global<Option<Card>> = Global::new(None);

/// The card interrupts the boot CPU when a frame arrives.
static INTERRUPTS: AtomicBool = AtomicBool::new(false);

/// The card is on a virtio-mmio transport.
static MMIO: AtomicBool = AtomicBool::new(false);

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
    let mut line = None;
    platform::for_each_virtio_mmio(|base, irq| {
        if card.is_some() {
            return;
        }
        match VirtioNet::probe(Mmio::new(base), DirectMap, alloc) {
            Ok(c) => {
                card = Some((format!("virtio-mmio {base:#x}"), c));
                line = irq;
                MMIO.store(true, Ordering::Relaxed);
            }
            Err(ProbeError::NotNet(_)) => {}
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
    if let Some(irq) = line
        && crate::arch::irq::route_device(irq)
    {
        card.enable_interrupts();
        INTERRUPTS.store(true, Ordering::Relaxed);
    }
    let m = card.mac();
    let _ = writeln!(
        ConsoleWriter(&mut BootConsole),
        "net: card {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} at {at}{}",
        m[0],
        m[1],
        m[2],
        m[3],
        m[4],
        m[5],
        if interrupts() { ", interrupts" } else { "" }
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
        let regions = match pci::net_regions(&mut Mmio::new(config), window.as_mut()) {
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

/// Longest time a card without interrupts goes unpolled while cells use
/// it and nothing else runs (design D4).
const BUSY_NS: u64 = 20_000;

/// Whether the card interrupts the boot CPU when a frame arrives.
pub fn interrupts() -> bool {
    INTERRUPTS.load(Ordering::Relaxed)
}

/// Whether the card is on a virtio-mmio transport.
#[cfg(feature = "ktest")]
pub fn on_mmio() -> bool {
    MMIO.load(Ordering::Relaxed)
}

/// Whether the card holds received frames not taken yet (memory only).
pub fn pending() -> bool {
    LINK.with(|l| l.as_ref().is_some_and(Card::pending))
}

/// Polls the card and `smoltcp`; returns in how many nanoseconds they want
/// to be polled again, if there is a card. A card that interrupts has its
/// interrupt acknowledged first, so a frame arriving later raises it again,
/// and is not polled for frames meanwhile.
pub fn poll(net: &mut Net, now_ns: u64) -> Option<u64> {
    LINK.with(|l| {
        let link = l.as_mut()?;
        let interrupts = interrupts();
        if interrupts {
            link.acknowledge();
        }
        let delay = net.poll_wire(link, now_ns).unwrap_or(u64::MAX);
        Some(if net.wire_busy() && !interrupts {
            delay.min(BUSY_NS)
        } else {
            delay
        })
    })
}
