//! The control channel over virtio-vsock (change `control-over-vsock`): a
//! control kernel listens on [`VSOCK_PORT`] of the VM's socket device, if
//! it has one, and the host's connection carries the control frames
//! instead of the serial line. A frame costs a notification of the device,
//! not two trapped register accesses per byte. The device is polled (from
//! the control loop), like the network card.

use crate::arch::console::BootConsole;
use crate::memory::DirectMap;
use crate::sync::Global;
use alloc::vec::Vec;
use core::fmt::Write;
use rhizo_control::VSOCK_PORT;
use rhizo_hal::ConsoleWriter;
use rhizo_virtio::vsock::{self, PAYLOAD_MAX, Stream, VirtioVsock};

struct Channel {
    dev: VirtioVsock<crate::arch::platform::Mmio, DirectMap>,
    stream: Stream,
    /// A packet's payload on its way out.
    out: Vec<u8>,
}

static CHANNEL: Global<Option<Channel>> = Global::new(None);

/// Brings up the first virtio socket device, if any, and listens on it.
pub fn init() {
    let Some((at, dev)) = crate::wire::find(
        rhizo_virtio::VSOCK_DEVICE,
        "vsock",
        vsock::FRAMES,
        |regs, alloc| VirtioVsock::probe(regs, DirectMap, alloc),
        |transport, alloc| VirtioVsock::probe_pci(transport, DirectMap, alloc),
    ) else {
        return;
    };
    let cid = dev.cid();
    let _ = writeln!(
        ConsoleWriter(&mut BootConsole),
        "vsock: cid {cid} port {VSOCK_PORT} at {at}"
    );
    CHANNEL.with(|c| {
        *c = Some(Channel {
            dev,
            stream: Stream::new(cid, VSOCK_PORT),
            out: Vec::new(),
        });
    });
}

/// Moves the packets the device received into the stream, and what the
/// stream has to send into the device while it has room.
fn pump(c: &mut Channel) {
    let Channel { dev, stream, out } = c;
    while dev.recv(&mut |h, data| {
        stream.take(h, data);
    }) {}
    while dev.can_send()
        && let Some(h) = stream.next_packet(PAYLOAD_MAX, out)
    {
        dev.send(&h, out);
    }
}

/// Polls the device when nothing received is left to read; whether the
/// host is connected.
pub fn poll() -> bool {
    CHANNEL.with(|c| {
        let Some(c) = c.as_mut() else {
            return false;
        };
        if !c.stream.has_input() {
            pump(c);
        }
        c.stream.connected()
    })
}

/// Whether the host is connected, as last polled.
pub fn connected() -> bool {
    CHANNEL.with(|c| c.as_ref().is_some_and(|c| c.stream.connected()))
}

/// The next byte the host sent, if one was received.
pub fn read_byte() -> Option<u8> {
    CHANNEL.with(|c| c.as_mut()?.stream.read())
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
