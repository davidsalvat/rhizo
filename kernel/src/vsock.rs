//! The control channel over virtio-vsock (change `control-over-vsock`): a
//! control kernel listens on [`VSOCK_PORT`] of the VM's socket device, if
//! it has one, and the host's connection carries the control frames
//! instead of the serial line. A frame costs a notification of the device,
//! not two trapped register accesses per byte. The device is polled (from
//! the control loop), like the network card.
//!
//! Any local user of the host may connect to a guest's vsock port on QEMU,
//! so a connection is served only once it presents a secret the kernel
//! drew at boot and gave on the serial line alone, which only the process
//! that started the VMM reads (change `authenticate-control-channel`).

use crate::arch::console::BootConsole;
use crate::arch::platform::{self, Mmio};
use crate::memory::{self, DirectMap};
use crate::sync::Global;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use rhizo_control::{SECRET_LEN, SECRET_LINE, VSOCK_PORT};
use rhizo_hal::ConsoleWriter;
use rhizo_virtio::vsock::{self, Gate, PAYLOAD_MAX, Stream, VirtioVsock};
use rhizo_virtio::{ProbeError, VSOCK_DEVICE, pci};

type Device = VirtioVsock<Mmio, DirectMap>;

struct Channel {
    dev: Device,
    stream: Gate,
    /// A packet's payload on its way out.
    out: Vec<u8>,
    /// Refused connections already reported on the console.
    reported: u32,
}

const _: () = assert!(SECRET_LEN == vsock::SECRET_LEN);

static CHANNEL: Global<Option<Channel>> = Global::new(None);

fn report(what: core::fmt::Arguments) {
    let _ = writeln!(ConsoleWriter(&mut BootConsole), "vsock: {what}");
}

/// Brings up the first virtio socket device, if any, and listens on it,
/// after giving the secret its connections present on the console (the
/// serial line). Without a hardware random source the secret could be
/// guessed: the device is left alone, and the frames stay on the serial
/// line.
pub fn init() {
    let Some((at, dev)) = find() else {
        return;
    };
    let mut secret = [0; SECRET_LEN];
    if !crate::cell::random_bytes(&mut secret) {
        report(format_args!(
            "no hardware random source: control stays on the serial line"
        ));
        return;
    }
    let cid = dev.cid();
    report(format_args!("cid {cid} port {VSOCK_PORT} at {at}"));
    let mut line = String::from(SECRET_LINE);
    for b in secret {
        let _ = write!(line, "{b:02x}");
    }
    let _ = writeln!(ConsoleWriter(&mut BootConsole), "{line}");
    CHANNEL.with(|c| {
        *c = Some(Channel {
            dev,
            stream: Gate::new(Stream::new(cid, VSOCK_PORT), secret),
            out: Vec::new(),
            reported: 0,
        });
    });
}

/// The first virtio socket device among the virtio-mmio transports the
/// platform lists, else on PCI bus 0, and where it is. As the network card
/// is found (`wire`), apart so that kernels without a control channel keep
/// their code as it was; PCI BARs left unassigned are not given addresses
/// (the firmware or the VMM assigns them wherever a control kernel runs).
fn find() -> Option<(String, Device)> {
    let mut run: Option<(u64, usize)> = None;
    let alloc = &mut || {
        if run.is_none() {
            let start = memory::with_frames(|f| f.alloc_contiguous(vsock::FRAMES)).flatten()?;
            run = Some((start, 0));
        }
        let (start, used) = run.as_mut()?;
        if *used == vsock::FRAMES {
            return None;
        }
        *used += 1;
        Some(*start + (*used as u64 - 1) * rhizo_mm::FRAME_SIZE)
    };
    let mut found = None;
    platform::for_each_virtio_mmio(|base| {
        if found.is_some() {
            return;
        }
        match VirtioVsock::probe(Mmio::new(base), DirectMap, alloc) {
            Ok(d) => found = Some((format!("virtio-mmio {base:#x}"), d)),
            Err(ProbeError::Other(_)) => {}
            Err(e) => report(format_args!("virtio-mmio at {base:#x}: {e:?}")),
        }
    });
    found.or_else(|| pci_device(alloc))
}

/// The first virtio socket device on PCI bus 0 (function 0 of each device).
fn pci_device(alloc: &mut dyn FnMut() -> Option<u64>) -> Option<(String, Device)> {
    let ecam = platform::pci_ecam()?;
    let regs = |(at, len)| platform::map_device(at, len).map(|()| Mmio::new(at));
    for device in 0..32u64 {
        let mut cfg = match regs((ecam + (device << 15), pci::FUNCTION_BYTES)) {
            Ok(cfg) => cfg,
            Err(e) => {
                report(format_args!("PCI bus 0: {e:?}"));
                return None;
            }
        };
        let r = match pci::regions(&mut cfg, VSOCK_DEVICE, None) {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            Err(e) => {
                report(format_args!("virtio-pci 00:{device:02x}.0: {e:?}"));
                continue;
            }
        };
        let probed = match (regs(r.common), regs(r.notify), regs(r.device)) {
            (Ok(c), Ok(n), Ok(d)) => {
                let transport = pci::Transport::new(c, n, r.notify_multiplier, d);
                VirtioVsock::probe_pci(transport, DirectMap, alloc).map_err(|e| format!("{e:?}"))
            }
            (Err(e), ..) | (_, Err(e), _) | (.., Err(e)) => Err(format!("{e:?}")),
        };
        match probed {
            Ok(d) => return Some((format!("virtio-pci 00:{device:02x}.0"), d)),
            Err(e) => report(format_args!("virtio-pci 00:{device:02x}.0: {e}")),
        }
    }
    None
}

/// Moves the packets the device received into the stream, resets a
/// connection that took too long to present the secret, and moves what the
/// stream has to send into the device while it has room.
fn pump(c: &mut Channel) {
    let Channel {
        dev,
        stream,
        out,
        reported,
    } = c;
    while dev.recv(&mut |h, data| {
        stream.take(h, data);
    }) {}
    stream.expire(crate::cell::now_ns());
    if stream.refused() != *reported {
        *reported = stream.refused();
        report(format_args!(
            "refused a connection without the secret ({} so far)",
            *reported
        ));
    }
    while dev.can_send()
        && let Some(h) = stream.next_packet(PAYLOAD_MAX, out)
    {
        dev.send(&h, out);
    }
}

/// Polls the device when nothing received is left to read; the number of
/// the host's connection, if it is connected and presented the secret
/// (`Gate::connection`).
pub fn poll() -> Option<u32> {
    CHANNEL.with(|c| {
        let c = c.as_mut()?;
        if !c.stream.has_input() {
            pump(c);
        }
        c.stream.connection()
    })
}

/// The number of the host's connection, as last polled.
pub fn connection() -> Option<u32> {
    CHANNEL.with(|c| c.as_ref()?.stream.connection())
}

/// The next byte the host sent on connection `connection`, if one was
/// received and that connection is still the host's.
pub fn read_byte(connection: u32) -> Option<u8> {
    CHANNEL.with(|c| c.as_mut()?.stream.read(connection))
}

/// Sends what is still queued for the host, polling the device, until the
/// device took every packet or `deadline` (monotonic nanoseconds) passes:
/// before the power off, so the last reply reaches the host.
pub fn finish(deadline: u64) {
    loop {
        let done = CHANNEL.with(|c| {
            let Some(c) = c.as_mut() else {
                return true;
            };
            pump(c);
            !c.stream.sending() && c.dev.sent_all()
        });
        if done || crate::cell::now_ns() >= deadline {
            return;
        }
        core::hint::spin_loop();
    }
}

/// Sends `bytes` (a frame) to the host; `false` if it is not connected.
pub fn write(bytes: &[u8]) -> bool {
    CHANNEL.with(|c| {
        let Some(c) = c.as_mut() else {
            return false;
        };
        let sent = c.stream.write(bytes);
        pump(c);
        sent
    })
}
