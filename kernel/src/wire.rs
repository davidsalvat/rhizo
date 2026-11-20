//! The network card (spec `networking`, design D1, D4 and D5 of
//! `add-virtio-net`): found among the virtio-mmio transports the platform
//! lists, given the address the command line asks for, and polled by the
//! cell host.

use crate::arch::console::BootConsole;
use crate::arch::platform::{self, Mmio};
use crate::memory::{self, DirectMap};
use crate::sync::Global;
use core::fmt::Write;
use rhizo_core::options::{BootOptions, IpOption};
use rhizo_hal::{ConsoleWriter, NetLink};
use rhizo_net::{Net, WireConfig};
use rhizo_virtio::{ProbeError, VirtioNet};

static LINK: Global<Option<VirtioNet<Mmio, DirectMap>>> = Global::new(None);

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
    platform::for_each_virtio_mmio(|base| {
        if card.is_some() {
            return;
        }
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
        match VirtioNet::probe(Mmio::new(base), DirectMap, alloc) {
            Ok(c) => card = Some((base, c)),
            Err(ProbeError::NotNet(_)) => {}
            Err(e) => {
                let _ = writeln!(
                    ConsoleWriter(&mut BootConsole),
                    "net: virtio-mmio at {base:#x}: {e:?}"
                );
            }
        }
    });
    let Some((base, mut card)) = card else {
        return;
    };
    let m = card.mac();
    let _ = writeln!(
        ConsoleWriter(&mut BootConsole),
        "net: card {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} at virtio-mmio {base:#x}",
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
