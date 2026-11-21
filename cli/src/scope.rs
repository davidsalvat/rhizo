//! `rhizo scope N --socket PATH` (spec `control-plane`, scoped host
//! access; design D14 of `add-cell-hierarchy`): serves on PATH the control
//! protocol limited to cell N's subtree, until it is interrupted. Each
//! client gets its own connection to the VM's server, and every request it
//! sends goes on as `scoped {N, request}`: the kernel refuses any whose
//! target is outside the subtree, that moves a cell out of it, or that
//! widens a policy or a limit. Replies come back as they are.

use crate::client::upgrade;
use crate::state::{Dir, Info};
use rhizo_control::{Decoder, Error, Frame, Item, Message, Request, VERSION};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::thread;

/// What to do with a client's frame: send these bytes on to the VM, or
/// back to the client (a refusal).
#[derive(Debug, PartialEq, Eq)]
pub enum Wrapped {
    On(Vec<u8>),
    Back(Vec<u8>),
}

/// The client's frame as a request scoped to host cell `root`'s subtree.
/// Its identifier stays: the client's connection to the server is its own.
pub fn wrap(frame: &Frame, root: u32) -> Wrapped {
    let refuse = |error: Error| {
        Wrapped::Back(
            Message::Failed {
                ns: 0,
                error,
                message: Vec::new(),
            }
            .encode(frame.id),
        )
    };
    match Request::decode(frame) {
        // A scoped request inside is the kernel's to refuse (never nested).
        Ok(request) => Wrapped::On(
            Request::Scoped {
                root,
                request: Box::new(request),
            }
            .encode(frame.id),
        ),
        Err(error) => refuse(error),
    }
}

/// Serves cell `root`'s subtree on `socket` until interrupted.
pub fn serve(dir: &Dir, root: u32, socket: &Path) -> Result<(), String> {
    let info = Info::read(dir).ok_or("no Rhizo VM is up: start one with `rhizo up`")?;
    match info.protocol {
        Some(VERSION) => {}
        other => {
            return Err(format!(
                "scoped access needs control protocol {VERSION}: {}",
                upgrade(other.unwrap_or(VERSION - 1))
            ));
        }
    }
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)
        .map_err(|e| format!("cannot listen on {}: {e}", socket.display()))?;
    println!(
        "[rhizo] serving cell {root}'s subtree on {}; Ctrl-C to stop",
        socket.display()
    );
    let server = dir.socket();
    for client in listener.incoming().flatten() {
        let server = server.clone();
        thread::spawn(move || {
            if let Ok(vm) = UnixStream::connect(&server) {
                relay(client, vm, root);
            }
        });
    }
    Ok(())
}

/// Relays one client: its requests scoped, the replies as they are.
fn relay(client: UnixStream, vm: UnixStream, root: u32) {
    let (Ok(mut back), Ok(mut replies)) = (client.try_clone(), vm.try_clone()) else {
        return;
    };
    let mut to_client = match client.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    };
    thread::spawn(move || {
        let _ = std::io::copy(&mut replies, &mut to_client);
    });
    let (mut from, mut to_vm) = (client, vm);
    let mut decoder = Decoder::any_version();
    let mut buf = [0u8; 4096];
    while let Ok(n) = from.read(&mut buf) {
        if n == 0 {
            break;
        }
        for &byte in &buf[..n] {
            let wrapped = match decoder.push(byte) {
                Some(Item::Frame(frame)) => wrap(&frame, root),
                Some(Item::Bad { id, error }) => Wrapped::Back(
                    Message::Failed {
                        ns: 0,
                        error,
                        message: Vec::new(),
                    }
                    .encode(id),
                ),
                Some(Item::Text(_)) | None => continue,
            };
            let sent = match wrapped {
                Wrapped::On(bytes) => to_vm.write_all(&bytes),
                Wrapped::Back(bytes) => back.write_all(&bytes),
            };
            if sent.is_err() {
                return;
            }
        }
    }
    let _ = to_vm.shutdown(std::net::Shutdown::Both);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhizo_control::PREVIOUS;

    fn frame(bytes: &[u8]) -> Frame {
        let mut d = Decoder::any_version();
        bytes
            .iter()
            .find_map(|&b| match d.push(b) {
                Some(Item::Frame(f)) => Some(f),
                _ => None,
            })
            .expect("a frame")
    }

    /// Every request goes on scoped to the subtree, under the client's
    /// identifier; what is not a request comes back refused; a scoped one
    /// goes on nested, for the kernel to refuse.
    #[test]
    fn requests_go_on_scoped() {
        let Wrapped::On(bytes) = wrap(&frame(&Request::KillTree(9).encode(7)), 3) else {
            panic!("not sent on");
        };
        let on = frame(&bytes);
        assert_eq!(on.id, 7);
        assert_eq!(
            Request::decode(&on),
            Ok(Request::Scoped {
                root: 3,
                request: Box::new(Request::KillTree(9))
            })
        );
        // A version-3 client's request is scoped too.
        let v3 = Request::Kill(9).encode_as(PREVIOUS, 8).unwrap();
        let Wrapped::On(bytes) = wrap(&frame(&v3), 3) else {
            panic!("not sent on");
        };
        assert!(matches!(
            Request::decode(&frame(&bytes)),
            Ok(Request::Scoped { root: 3, .. })
        ));
        // A reply is no request.
        let reply = Message::Hello { version: 4 }.encode(5);
        let Wrapped::Back(bytes) = wrap(&frame(&reply), 3) else {
            panic!("sent on");
        };
        assert!(matches!(
            Message::decode(&frame(&bytes)),
            Ok(Message::Failed {
                error: Error::UnknownKind,
                ..
            })
        ));
        // Nested: the kernel refuses it when it reads it.
        let nested = Request::Scoped {
            root: 9,
            request: Box::new(Request::List),
        }
        .encode(6);
        let Wrapped::On(bytes) = wrap(&frame(&nested), 3) else {
            panic!("a client's scoped request was not sent on");
        };
        assert_eq!(Request::decode(&frame(&bytes)), Err(Error::Malformed));
    }
}
