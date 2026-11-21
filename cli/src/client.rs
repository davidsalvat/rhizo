//! A connection to the background server, which relays requests to the
//! control kernel.

use crate::state::{Dir, Info};
use rhizo_control::{Body, Decoder, Item, Message, Request};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct Client {
    stream: UnixStream,
    decoder: Decoder,
    next_id: u32,
    /// Messages read while waiting for another request's reply.
    pending: VecDeque<(u32, Message)>,
    pub info: Info,
}

/// A successful reply: its body, the kernel's time and the round trip.
#[derive(Debug)]
pub struct Reply {
    pub body: Body,
    pub ns: u64,
    pub round_trip: Duration,
}

impl Reply {
    /// `<what> in <kernel time> (round trip <host time>)`.
    pub fn timing(&self) -> String {
        format!(
            "{} (round trip {})",
            duration(self.ns),
            duration(self.round_trip.as_nanos() as u64)
        )
    }
}

/// A duration for people: microseconds below 10 ms, else milliseconds.
pub fn duration(ns: u64) -> String {
    if ns < 10_000_000 {
        format!("{:.0} us", ns as f64 / 1e3)
    } else {
        format!("{:.1} ms", ns as f64 / 1e6)
    }
}

impl Client {
    /// Connects to the VM `rhizo up` started.
    pub fn connect(dir: &Dir) -> Result<Self, String> {
        let info = Info::read(dir).ok_or_else(|| {
            format!(
                "no Rhizo VM is up (state {}): start one with `rhizo up`",
                dir.0.display()
            )
        })?;
        let stream = UnixStream::connect(dir.socket())
            .map_err(|e| format!("cannot reach the VM at {}: {e}", dir.socket().display()))?;
        Ok(Client {
            stream,
            decoder: Decoder::new(),
            next_id: 1,
            pending: VecDeque::new(),
            info,
        })
    }

    /// Sends `request`; returns its identifier.
    pub fn send(&mut self, request: &Request) -> Result<u32, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.stream
            .write_all(&request.encode(id))
            .map_err(|e| format!("the VM's server is gone: {e}"))?;
        Ok(id)
    }

    /// The next message from the kernel, with the identifier of the request
    /// it answers.
    pub fn next(&mut self) -> Result<(u32, Message), String> {
        if let Some(m) = self.pending.pop_front() {
            return Ok(m);
        }
        let mut buf = [0u8; 4096];
        loop {
            let n = self
                .stream
                .read(&mut buf)
                .map_err(|e| format!("lost the VM: {e}"))?;
            if n == 0 {
                return Err("the VM stopped".into());
            }
            for &byte in &buf[..n] {
                match self.decoder.push(byte) {
                    Some(Item::Frame(frame)) => match Message::decode(&frame) {
                        Ok(m) => self.pending.push_back((frame.id, m)),
                        Err(e) => return Err(format!("unreadable reply: {}", e.as_str())),
                    },
                    Some(Item::Bad { error, .. }) => {
                        return Err(format!("unreadable reply: {}", error.as_str()));
                    }
                    Some(Item::Text(_)) | None => {}
                }
            }
            if let Some(m) = self.pending.pop_front() {
                return Ok(m);
            }
        }
    }

    /// Sends `request` and waits for its reply; other messages wait in
    /// line for [`next`](Self::next). A refusal is an error.
    pub fn call(&mut self, request: &Request) -> Result<Reply, String> {
        let t0 = Instant::now();
        let id = self.send(request)?;
        let mut other = VecDeque::new();
        let reply = loop {
            match self.next()? {
                (got, message) if got == id => break message,
                m => other.push_back(m),
            }
        };
        let round_trip = t0.elapsed();
        other.append(&mut self.pending);
        self.pending = other;
        match reply {
            Message::Done { ns, body } => Ok(Reply {
                body,
                ns,
                round_trip,
            }),
            Message::Failed { error, message, .. } => Err(crate::refusal(error, &message)),
            other => Err(format!("unexpected reply {other:?}")),
        }
    }
}
