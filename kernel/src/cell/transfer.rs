//! The network card as the host uses it: polling it while cells run,
//! and snapshots sent to and fetched from the host through it.

use super::*;

impl Host {
    /// Polls the network card when it is due (every scheduling step at most
    /// once per [`WIRE_PERIOD_NS`]), and reports a new address.
    pub(super) fn poll_wire(&mut self, now: u64) {
        if now < self.wire_due {
            return;
        }
        let _p = cost::scope(Part::Net);
        let (delay, addr) = with_net(|net| (crate::wire::poll(net, now), net.wire_addr()));
        // As often as the wire needs, but not more than once per period,
        // and at least every millisecond for frames nobody waits for yet.
        self.wire_due = now + delay.unwrap_or(u64::MAX).clamp(WIRE_PERIOD_NS, 1_000_000);
        self.wire_delay = delay;
        self.wire_addr = addr;
        let packed = addr.map_or(0, u32::from_be_bytes);
        if WIRE_ADDR.swap(packed, Ordering::Relaxed) != packed
            && let Some(a) = addr
        {
            use core::fmt::Write;
            let _ = writeln!(
                rhizo_hal::ConsoleWriter(&mut BootConsole),
                "net: address {}.{}.{}.{}",
                a[0],
                a[1],
                a[2],
                a[3]
            );
        }
    }

    /// Polls the network card until it has an address, at most
    /// `timeout_ns`; returns the address.
    #[cfg_attr(
        not(feature = "ktest"),
        allow(dead_code, reason = "used by in-kernel tests and benchmarks")
    )]
    pub fn wire_address(&mut self, timeout_ns: u64) -> Option<[u8; 4]> {
        let end = now_ns().saturating_add(timeout_ns);
        loop {
            let now = now_ns();
            self.wire_due = 0;
            self.poll_wire(now);
            if self.wire_addr.is_some() || self.wire_delay.is_none() || now >= end {
                return self.wire_addr;
            }
            idle_until(now + WIRE_PERIOD_NS * 50);
        }
    }

    /// Sends `bytes` to `port` at `ip` over TCP through the network card,
    /// and returns once the host has them all and closed its end (design
    /// D2 of `snapshots-on-disk`: how a snapshot file leaves the kernel).
    #[cfg_attr(
        not(feature = "control"),
        allow(dead_code, reason = "used by the control kernel")
    )]
    pub fn send_bytes(
        &mut self,
        ip: [u8; 4],
        port: u16,
        token: u64,
        bytes: &[u8],
    ) -> Result<(), Transfer> {
        let mut sent = 0;
        let mut shut = false;
        self.stream(ip, port, token, |net, h| {
            if sent < bytes.len() {
                return match net.send(h, &bytes[sent..], None) {
                    Ok(n) => {
                        sent += n;
                        Ok(Some(n > 0))
                    }
                    Err(rhizo_net::Error::Again) => Ok(Some(false)),
                    Err(e) => Err(Transfer::Net(e)),
                };
            }
            if !shut {
                shut = true;
                net.shutdown(h, rhizo_net::Shutdown::Write)
                    .map_err(Transfer::Net)?;
            }
            // Done when the host, having read everything, closes.
            match net.recv(h, 1, false, &mut |_| true) {
                Ok(r) if r.len == 0 => Ok(None),
                Ok(_) | Err(rhizo_net::Error::Again) => Ok(Some(false)),
                Err(e) => Err(Transfer::Net(e)),
            }
        })
    }

    /// Receives `size` bytes from `port` at `ip` over TCP through the
    /// network card (design D2 of `snapshots-on-disk`: how a snapshot file
    /// enters the kernel), into `out`, which has room for them.
    #[cfg_attr(
        not(feature = "control"),
        allow(dead_code, reason = "used by the control kernel")
    )]
    pub fn fetch_bytes(
        &mut self,
        ip: [u8; 4],
        port: u16,
        token: u64,
        size: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), Transfer> {
        self.stream(ip, port, token, |net, h| {
            if out.len() == size {
                return Ok(None);
            }
            let left = size - out.len();
            match net.recv(h, left, false, &mut |b| {
                out.extend_from_slice(b);
                true
            }) {
                Ok(r) if r.len == 0 => Err(Transfer::Short),
                Ok(_) => Ok(Some(true)),
                Err(rhizo_net::Error::Again) => Ok(Some(false)),
                Err(e) => Err(Transfer::Net(e)),
            }
        })
    }

    /// Connects to `port` at `ip`, sends `token` (the request's, which the
    /// host checks before it sends or keeps a byte), and polls the card and
    /// `step` until
    /// `step` says the transfer is done (`None`), fails, or makes no
    /// progress for [`TRANSFER_STALL_NS`]; the socket is released either
    /// way. `step` says whether it moved bytes.
    fn stream(
        &mut self,
        ip: [u8; 4],
        port: u16,
        token: u64,
        mut step: impl FnMut(&mut Net, rhizo_net::Handle) -> Result<Option<bool>, Transfer>,
    ) -> Result<(), Transfer> {
        use rhizo_net::{Addr, Error, Family, Type};
        let h = with_net(|net| net.socket(Family::Inet, Type::Stream)).map_err(Transfer::Net)?;
        let to = Addr::Inet { ip, port };
        let (mut connected, mut last) = (false, now_ns());
        let token = token.to_le_bytes();
        let mut told = 0;
        let result = loop {
            let now = now_ns();
            if now - last > TRANSFER_STALL_NS {
                break Err(Transfer::Stalled);
            }
            self.wire_due = 0;
            self.poll_wire(now);
            let moved = with_net(|net| {
                if !connected {
                    return match net.connect(h, &to) {
                        Ok(()) => {
                            connected = true;
                            Ok(Some(true))
                        }
                        Err(Error::InProgress | Error::Again) => Ok(Some(false)),
                        Err(e) => Err(Transfer::Net(e)),
                    };
                }
                if told < token.len() {
                    return match net.send(h, &token[told..], None) {
                        Ok(n) => {
                            told += n;
                            Ok(Some(n > 0))
                        }
                        Err(Error::Again) => Ok(Some(false)),
                        Err(e) => Err(Transfer::Net(e)),
                    };
                }
                step(net, h)
            });
            match moved {
                Ok(None) => break Ok(()),
                Ok(Some(true)) => last = now,
                Ok(Some(false)) => core::hint::spin_loop(),
                Err(e) => break Err(e),
            }
        };
        with_net(|net| net.release(h));
        result
    }
}
