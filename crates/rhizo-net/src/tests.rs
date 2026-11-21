//! Host tests of the socket layer, through its public operations.

use super::*;
use alloc::vec;
use proptest::prelude::*;

fn inet(ip: [u8; 4], port: u16) -> Addr {
    Addr::Inet { ip, port }
}

fn unix(name: u64) -> Addr {
    Addr::Unix {
        name,
        path: b"sock".to_vec(),
    }
}

/// Everything `h` holds now (stream), or `Err`.
fn drain(net: &mut Net, h: Handle) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    net.recv(h, usize::MAX, false, &mut |b| {
        out.extend_from_slice(b);
        true
    })?;
    Ok(out)
}

fn listener(net: &mut Net, port: u16) -> Handle {
    let l = net.socket(Family::Inet, Type::Stream).unwrap();
    net.bind(l, &inet(LOOPBACK, port)).unwrap();
    net.listen(l, 8).unwrap();
    l
}

/// A connected pair `(client, server)` on `port`, with listener.
fn connection(net: &mut Net, port: u16) -> (Handle, Handle, Handle) {
    let l = listener(net, port);
    let c = net.socket(Family::Inet, Type::Stream).unwrap();
    net.connect(c, &inet(LOOPBACK, port)).unwrap();
    let s = net.accept(l).unwrap();
    (c, s, l)
}

#[test]
fn tcp_echo_over_loopback() {
    let mut net = Net::new();
    let (c, s, l) = connection(&mut net, 7000);
    assert_eq!(net.accept(l), Err(Error::Again), "one connection only");
    assert_eq!(net.send(c, b"ping", None), Ok(4));
    assert_eq!(drain(&mut net, s).unwrap(), b"ping");
    assert_eq!(drain(&mut net, s), Err(Error::Again), "nothing more yet");
    assert_eq!(net.send(s, b"PONG", None), Ok(4));
    assert_eq!(drain(&mut net, c).unwrap(), b"PONG");
    let client = net.local_addr(c).unwrap().unwrap();
    assert!(matches!(client, Addr::Inet { ip: LOOPBACK, port } if EPHEMERAL.contains(&port)));
    assert_eq!(net.peer_addr(s), Ok(client));
    assert_eq!(net.peer_addr(c), Ok(inet(LOOPBACK, 7000)));
    assert_eq!(net.local_addr(s), Ok(Some(inet(LOOPBACK, 7000))));
    assert_eq!(net.peer_addr(l), Err(Error::NotConn));
}

#[test]
fn listener_on_any_address_takes_every_loopback_address() {
    let mut net = Net::new();
    let l = net.socket(Family::Inet, Type::Stream).unwrap();
    net.bind(l, &inet(ANY, 80)).unwrap();
    net.listen(l, 1).unwrap();
    let c = net.socket(Family::Inet, Type::Stream).unwrap();
    net.connect(c, &inet([127, 1, 2, 3], 80)).unwrap();
    let s = net.accept(l).unwrap();
    assert_eq!(net.local_addr(s), Ok(Some(inet([127, 1, 2, 3], 80))));
}

#[test]
fn only_loopback_addresses_exist() {
    let mut net = Net::new();
    let s = net.socket(Family::Inet, Type::Stream).unwrap();
    assert_eq!(
        net.bind(s, &inet([10, 0, 0, 1], 1)),
        Err(Error::AddrNotAvail)
    );
    assert_eq!(
        net.connect(s, &inet([10, 0, 0, 1], 1)),
        Err(Error::NetUnreach)
    );
    assert_eq!(net.connect(s, &inet(LOOPBACK, 1)), Err(Error::ConnRefused));
    let u = net.socket(Family::Inet, Type::Dgram).unwrap();
    assert_eq!(
        net.send(u, b"x", Some(&inet([8, 8, 8, 8], 53))),
        Err(Error::NetUnreach)
    );
    assert_eq!(net.send(u, b"x", None), Err(Error::DestAddrReq));
}

#[test]
fn ports_are_unique_unless_shared_on_purpose() {
    let mut net = Net::new();
    let l = listener(&mut net, 8080);
    let other = net.socket(Family::Inet, Type::Stream).unwrap();
    assert_eq!(net.bind(other, &inet(ANY, 8080)), Err(Error::AddrInUse));
    let mut reuse = net.options(other).unwrap();
    reuse.reuse_addr = true;
    net.set_options(other, reuse).unwrap();
    assert_eq!(
        net.bind(other, &inet(LOOPBACK, 8080)),
        Err(Error::AddrInUse),
        "a listener is never shared through SO_REUSEADDR"
    );
    let udp = net.socket(Family::Inet, Type::Dgram).unwrap();
    assert_eq!(
        net.bind(udp, &inet(LOOPBACK, 8080)),
        Ok(()),
        "UDP ports are apart"
    );
    net.release(l);
    assert_eq!(net.bind(other, &inet(LOOPBACK, 8080)), Ok(()));
    assert_eq!(
        net.bind(other, &inet(LOOPBACK, 8081)),
        Err(Error::Invalid),
        "bound once"
    );
}

#[test]
fn reuse_addr_rebinds_a_port_whose_connections_remain() {
    let mut net = Net::new();
    let l = net.socket(Family::Inet, Type::Stream).unwrap();
    let mut o = net.options(l).unwrap();
    o.reuse_addr = true;
    net.set_options(l, o).unwrap();
    net.bind(l, &inet(LOOPBACK, 9000)).unwrap();
    net.listen(l, 4).unwrap();
    let c = net.socket(Family::Inet, Type::Stream).unwrap();
    net.connect(c, &inet(LOOPBACK, 9000)).unwrap();
    let _accepted = net.accept(l).unwrap();
    net.release(l);
    let again = net.socket(Family::Inet, Type::Stream).unwrap();
    net.set_options(again, o).unwrap();
    assert_eq!(net.bind(again, &inet(LOOPBACK, 9000)), Ok(()));
    let plain = net.socket(Family::Inet, Type::Stream).unwrap();
    assert_eq!(
        net.bind(plain, &inet(LOOPBACK, 9000)),
        Err(Error::AddrInUse)
    );
}

#[test]
fn port_zero_picks_ephemeral_ports_in_order() {
    let mut net = Net::new();
    let a = net.socket(Family::Inet, Type::Stream).unwrap();
    let b = net.socket(Family::Inet, Type::Stream).unwrap();
    net.bind(a, &inet(LOOPBACK, 0)).unwrap();
    net.bind(b, &inet(LOOPBACK, 0)).unwrap();
    assert_eq!(net.local_addr(a), Ok(Some(inet(LOOPBACK, 32768))));
    assert_eq!(net.local_addr(b), Ok(Some(inet(LOOPBACK, 32769))));
    let l = net.socket(Family::Inet, Type::Stream).unwrap();
    net.listen(l, 1).unwrap();
    assert_eq!(
        net.local_addr(l),
        Ok(Some(inet(ANY, 32770))),
        "listen binds"
    );
}

#[test]
fn closing_ends_the_stream_or_resets_it() {
    let mut net = Net::new();
    let (c, s, _) = connection(&mut net, 1);
    net.send(c, b"bye", None).unwrap();
    net.release(c);
    assert_eq!(
        drain(&mut net, s).unwrap(),
        b"bye",
        "data sent before a close arrives"
    );
    assert_eq!(
        drain(&mut net, s).unwrap(),
        b"",
        "then the end of the stream"
    );
    assert_eq!(net.send(s, b"x", None), Err(Error::Pipe));
    assert_eq!(net.poll(s) & (IN | RDHUP | HUP), IN | RDHUP | HUP);

    let (c, s, _) = connection(&mut net, 2);
    net.send(c, b"unread", None).unwrap();
    net.release(s);
    assert_eq!(
        drain(&mut net, c),
        Err(Error::ConnReset),
        "closed with unread data"
    );
    assert_eq!(drain(&mut net, c).unwrap(), b"", "reported once");
}

#[test]
fn closing_a_listener_resets_connections_never_accepted() {
    let mut net = Net::new();
    let l = listener(&mut net, 3);
    let c = net.socket(Family::Inet, Type::Stream).unwrap();
    net.connect(c, &inet(LOOPBACK, 3)).unwrap();
    let open = net.open();
    net.release(l);
    assert_eq!(net.open(), open - 2, "the listener and the queued end");
    assert_eq!(net.poll(c) & ERR, ERR);
    assert_eq!(net.send(c, b"x", None), Err(Error::ConnReset));
}

#[test]
fn shutdown_write_is_an_end_of_stream_for_the_peer() {
    let mut net = Net::new();
    let (c, s, _) = connection(&mut net, 4);
    net.shutdown(c, Shutdown::Write).unwrap();
    assert_eq!(net.send(c, b"x", None), Err(Error::Pipe));
    assert_eq!(net.poll(s) & (IN | RDHUP), IN | RDHUP);
    assert_eq!(drain(&mut net, s).unwrap(), b"");
    assert_eq!(
        net.send(s, b"still", None),
        Ok(5),
        "the other direction works"
    );
    assert_eq!(drain(&mut net, c).unwrap(), b"still");
    let l = listener(&mut net, 5);
    assert_eq!(net.shutdown(l, Shutdown::Both), Err(Error::NotConn));
}

#[test]
fn backlog_bounds_waiting_connections() {
    let mut net = Net::new();
    let l = net.socket(Family::Inet, Type::Stream).unwrap();
    net.bind(l, &inet(LOOPBACK, 6)).unwrap();
    net.listen(l, 1).unwrap();
    for _ in 0..2 {
        let c = net.socket(Family::Inet, Type::Stream).unwrap();
        net.connect(c, &inet(LOOPBACK, 6)).unwrap();
    }
    let c = net.socket(Family::Inet, Type::Stream).unwrap();
    assert_eq!(net.connect(c, &inet(LOOPBACK, 6)), Err(Error::ConnRefused));
    assert_eq!(net.connect(c, &inet(LOOPBACK, 6)), Err(Error::ConnRefused));
    net.accept(l).unwrap();
    assert_eq!(net.connect(c, &inet(LOOPBACK, 6)), Ok(()));
    assert_eq!(net.connect(c, &inet(LOOPBACK, 6)), Err(Error::IsConn));
}

#[test]
fn a_full_stream_refuses_more_until_read() {
    let mut net = Net::new();
    let (c, s, _) = connection(&mut net, 7);
    let big = vec![7u8; STREAM_BUF + 10];
    assert_eq!(net.send(c, &big, None), Ok(STREAM_BUF));
    assert_eq!(net.poll(c) & OUT, 0);
    assert_eq!(net.send(c, b"x", None), Err(Error::Again));
    let before = net.changes();
    let mut got = 0;
    net.recv(s, 100, false, &mut |b| {
        got += b.len();
        true
    })
    .unwrap();
    assert_eq!(got, 100);
    assert!(net.changes() > before, "room for a waiting writer");
    assert_eq!(net.poll(c) & OUT, OUT);
}

#[test]
fn peek_leaves_the_bytes() {
    let mut net = Net::new();
    let (c, s, _) = connection(&mut net, 8);
    net.send(c, b"abc", None).unwrap();
    let mut seen = Vec::new();
    net.recv(s, 2, true, &mut |b| {
        seen.extend_from_slice(b);
        true
    })
    .unwrap();
    assert_eq!(seen, b"ab");
    assert_eq!(drain(&mut net, s).unwrap(), b"abc");
}

#[test]
fn refused_take_consumes_nothing() {
    let mut net = Net::new();
    let (c, s, _) = connection(&mut net, 9);
    net.send(c, b"abc", None).unwrap();
    let got = net.recv(s, 3, false, &mut |_| false).unwrap();
    assert_eq!(got.len, 0);
    assert_eq!(drain(&mut net, s).unwrap(), b"abc");
}

#[test]
fn udp_datagrams_keep_their_bounds_and_sender() {
    let mut net = Net::new();
    let a = net.socket(Family::Inet, Type::Dgram).unwrap();
    let b = net.socket(Family::Inet, Type::Dgram).unwrap();
    net.bind(b, &inet(LOOPBACK, 5353)).unwrap();
    assert_eq!(net.poll(b) & IN, 0);
    assert_eq!(net.send(a, b"one", Some(&inet(LOOPBACK, 5353))), Ok(3));
    assert_eq!(net.send(a, b"second", Some(&inet(LOOPBACK, 5353))), Ok(6));
    assert_eq!(
        net.send(a, b"lost", Some(&inet(LOOPBACK, 5354))),
        Ok(4),
        "nobody there"
    );
    assert_eq!(net.poll(b) & IN, IN);
    let Some(Addr::Inet { ip: ANY, port }) = net.local_addr(a).unwrap() else {
        panic!("sending binds to any address and a port");
    };
    let from = inet(LOOPBACK, port);
    let mut data = Vec::new();
    let got = net
        .recv(b, 2, false, &mut |d| {
            data.extend_from_slice(d);
            true
        })
        .unwrap();
    assert_eq!(
        (data.as_slice(), got.full, got.from),
        (&b"on"[..], 3, Some(from.clone()))
    );
    let got = net.recv(b, 100, false, &mut |_| true).unwrap();
    assert_eq!((got.len, got.full), (6, 6), "the cut datagram is gone");
    assert_eq!(net.recv(b, 100, false, &mut |_| true), Err(Error::Again));
    assert_eq!(
        net.send(b, &vec![0; DGRAM_MAX + 1], Some(&from)),
        Err(Error::MsgSize)
    );
}

#[test]
fn connected_udp_takes_only_its_peer() {
    let mut net = Net::new();
    let a = net.socket(Family::Inet, Type::Dgram).unwrap();
    net.bind(a, &inet(LOOPBACK, 4000)).unwrap();
    let b = net.socket(Family::Inet, Type::Dgram).unwrap();
    net.connect(b, &inet(LOOPBACK, 4000)).unwrap();
    net.connect(a, &net.local_addr(b).unwrap().unwrap())
        .unwrap();
    let stranger = net.socket(Family::Inet, Type::Dgram).unwrap();
    net.send(stranger, b"no", Some(&inet(LOOPBACK, 4000)))
        .unwrap();
    net.send(b, b"yes", None).unwrap();
    let mut data = Vec::new();
    net.recv(a, 10, false, &mut |d| {
        data.extend_from_slice(d);
        true
    })
    .unwrap();
    assert_eq!(data, b"yes");
    assert_eq!(net.recv(a, 10, false, &mut |_| true), Err(Error::Again));
    assert_eq!(net.peer_addr(b), Ok(inet(LOOPBACK, 4000)));
}

#[test]
fn unix_sockets_by_name() {
    let mut net = Net::new();
    let name = net.new_name();
    let l = net.socket(Family::Unix, Type::Stream).unwrap();
    net.bind(l, &unix(name)).unwrap();
    net.listen(l, 4).unwrap();
    let c = net.socket(Family::Unix, Type::Stream).unwrap();
    net.connect(c, &unix(name)).unwrap();
    let s = net.accept(l).unwrap();
    net.send(s, b"hi", None).unwrap();
    assert_eq!(drain(&mut net, c).unwrap(), b"hi");
    assert_eq!(net.peer_addr(c), Ok(unix(name)));
    assert_eq!(net.local_addr(c), Ok(Some(Addr::UnixUnnamed)));
    net.release(l);
    let d = net.socket(Family::Unix, Type::Stream).unwrap();
    assert_eq!(net.connect(d, &unix(name)), Err(Error::ConnRefused));
    assert_eq!(net.socket(Family::Unix, Type::Dgram), Err(Error::OpNotSupp));
    assert_eq!(net.connect(d, &inet(LOOPBACK, 1)), Err(Error::Invalid));
}

#[test]
fn socket_pairs_are_connected() {
    let mut net = Net::new();
    let (a, b) = net.pair().unwrap();
    net.send(a, b"x", None).unwrap();
    assert_eq!(drain(&mut net, b).unwrap(), b"x");
    net.release(a);
    assert_eq!(drain(&mut net, b).unwrap(), b"");
}

#[test]
fn readiness_of_fresh_sockets_is_linux_s() {
    let mut net = Net::new();
    let s = net.socket(Family::Inet, Type::Stream).unwrap();
    assert_eq!(net.poll(s), OUT | HUP);
    let u = net.socket(Family::Inet, Type::Dgram).unwrap();
    assert_eq!(net.poll(u), OUT);
    let (c, _, l) = connection(&mut net, 10);
    assert_eq!(net.poll(c), OUT);
    assert_eq!(net.poll(l), 0);
}

#[test]
fn changes_count_what_waiters_wait_for() {
    let mut net = Net::new();
    let l = listener(&mut net, 11);
    let before = net.changes();
    let c = net.socket(Family::Inet, Type::Stream).unwrap();
    net.connect(c, &inet(LOOPBACK, 11)).unwrap();
    assert!(net.changes() > before, "a connection to accept");
    let s = net.accept(l).unwrap();
    let before = net.changes();
    net.send(c, b"x", None).unwrap();
    assert!(net.changes() > before, "data to read");
    let before = net.changes();
    net.release(s);
    assert!(net.changes() > before, "a close");
}

/// A copy's sockets live in its own network (design D5 of
/// `add-cell-hierarchy`): its listener is bound again there, a fresh socket
/// is copied, a connection is reset; the source keeps all of its own, and
/// no client of one network reaches the other's listener.
#[test]
fn networks_of_copies_bind_listeners_again() {
    let mut net = Net::new();
    let (c, s, l) = connection(&mut net, 12);
    let copy_l = net.fork(l, 5).unwrap();
    assert_ne!(copy_l, l, "a listener of its own");
    assert!(net.is_listening(copy_l));
    assert_eq!(net.local_addr(copy_l), Ok(Some(inet(LOOPBACK, 12))));
    assert_eq!(net.network(copy_l), Ok(5));
    assert_eq!(net.network(l), Ok(0), "the source's is unchanged");
    let fresh = net.socket(Family::Inet, Type::Stream).unwrap();
    let copy = net.fork(fresh, 5).unwrap();
    assert_ne!(copy, fresh);
    assert_eq!(net.poll(copy), OUT | HUP);
    let reset = net.fork(s, 5).unwrap();
    assert_reset_sequence(&mut net, reset);
    net.send(c, b"still", None).unwrap();
    assert_eq!(
        drain(&mut net, s).unwrap(),
        b"still",
        "the source keeps its connection"
    );
    // A client of the copy's network reaches the copy's listener only.
    net.enter(Ports::new(5));
    let c5 = net.socket(Family::Inet, Type::Stream).unwrap();
    net.leave();
    net.connect(c5, &inet(LOOPBACK, 12)).unwrap();
    assert_eq!(net.accept(l), Err(Error::Again), "not the source's");
    assert!(net.accept(copy_l).is_ok());
    // Closing the source's listener closes its port, not the copy's.
    net.release(l);
    let c0 = net.socket(Family::Inet, Type::Stream).unwrap();
    assert_eq!(
        net.connect(c0, &inet(LOOPBACK, 12)),
        Err(Error::ConnRefused)
    );
    assert_eq!(net.connect(copy, &inet(LOOPBACK, 12)), Ok(()));
    assert!(net.accept(copy_l).is_ok());
}

#[test]
fn snapshot_records_park_listeners_and_connections() {
    let mut net = Net::new();
    let (c, s, l) = connection(&mut net, 13);
    let mut o = net.options(l).unwrap();
    o.no_delay = true;
    net.set_options(l, o).unwrap();
    net.send(c, b"before", None).unwrap();
    let (rl, rs) = (net.export(l).unwrap(), net.export(s).unwrap());
    assert_eq!(rl.state, Saved::Listening(8));
    assert_eq!(rl.local, Some(inet(LOOPBACK, 13)));
    assert!(rl.parked.is_some());
    assert_eq!((rs.state, &rs.local), (Saved::Reset, &None));
    assert!(rs.parked.is_some());
    // The cell is suspended: its references go.
    net.release(l);
    net.release(s);
    let waiting = net.socket(Family::Inet, Type::Stream).unwrap();
    assert_eq!(
        net.connect(waiting, &inet(LOOPBACK, 13)),
        Ok(()),
        "a parked listener queues connections"
    );
    assert_eq!(net.poll(c) & (ERR | HUP), 0, "the peer sees nothing");
    assert_eq!(net.send(c, b" during", None), Ok(7));
    let (l2, renamed) = net.import(&rl, 0).unwrap();
    assert_eq!((l2, renamed), (l, None), "the same listener");
    assert!(net.options(l2).unwrap().no_delay);
    assert!(net.accept(l2).is_ok(), "the connection made meanwhile");
    let (s2, _) = net.import(&rs, 0).unwrap();
    assert_eq!(s2, s);
    assert_eq!(drain(&mut net, s2).unwrap(), b"before during");
    assert_eq!(net.send(s2, b"back", None), Ok(4));
    assert_eq!(drain(&mut net, c).unwrap(), b"back", "the same peer");
    assert_eq!(net.peer_addr(s2), net.local_addr(c).map(Option::unwrap));
    let (taken, _) = net.import(&rl, 0).unwrap();
    assert_ne!(taken, l);
    assert_eq!(
        net.poll(taken) & ERR,
        ERR,
        "parked no more, address taken: reset"
    );
}

#[test]
fn a_listener_not_parked_here_is_bound_again() {
    let mut net = Net::new();
    net.set_boot(1);
    let l = listener(&mut net, 19);
    let r = net.export(l).unwrap();
    net.release(l);
    let mut other = Net::new();
    other.set_boot(2);
    let (l2, _) = other.import(&r, 0).unwrap();
    assert!(other.is_listening(l2));
    let c = other.socket(Family::Inet, Type::Stream).unwrap();
    other.connect(c, &inet(LOOPBACK, 19)).unwrap();
    assert!(other.accept(l2).is_ok());
}

/// The calls on a reset connection: `ECONNRESET` once, then the end of the
/// stream for reads and `EPIPE` for writes.
fn assert_reset_sequence(net: &mut Net, h: Handle) {
    assert_eq!(net.poll(h), IN | OUT | HUP | RDHUP | ERR);
    assert_eq!(drain(net, h), Err(Error::ConnReset));
    assert_eq!(net.poll(h), IN | OUT | HUP | RDHUP);
    assert_eq!(drain(net, h).unwrap(), b"");
    assert_eq!(net.send(h, b"x", None), Err(Error::Pipe));
    assert_eq!(net.peer_addr(h), Err(Error::NotConn));
}

#[test]
fn a_parked_connection_comes_back_once() {
    let mut net = Net::new();
    let (c, s, _) = connection(&mut net, 14);
    let r = net.export(s).unwrap();
    net.release(s);
    let (first, _) = net.import(&r, 0).unwrap();
    assert_eq!(first, s, "the same socket");
    let (second, _) = net.import(&r, 0).unwrap();
    assert_ne!(second, s);
    assert_reset_sequence(&mut net, second);
    assert_eq!(net.send(c, b"ok", None), Ok(2));
    assert_eq!(drain(&mut net, first).unwrap(), b"ok");
}

#[test]
fn a_parked_connection_comes_back_only_in_its_boot() {
    let mut net = Net::new();
    net.set_boot(1);
    let (c, s, _) = connection(&mut net, 15);
    let r = net.export(s).unwrap();
    net.release(s);
    let Some(Park { boot: 1, key }) = r.parked else {
        panic!("parked in boot 1: {:?}", r.parked);
    };
    let mut other = Net::new();
    other.set_boot(2);
    let (h, _) = other.import(&r, 0).unwrap();
    assert_reset_sequence(&mut other, h);
    let elsewhere = Record {
        parked: Some(Park { boot: 2, key }),
        ..r.clone()
    };
    let (h, _) = net.import(&elsewhere, 0).unwrap();
    assert_reset_sequence(&mut net, h);
    let (back, _) = net.import(&r, 0).unwrap();
    assert_eq!(back, s, "still parked for its own boot");
    net.send(c, b"x", None).unwrap();
    assert_eq!(drain(&mut net, back).unwrap(), b"x");
}

#[test]
fn a_parked_connection_buffers_up_to_its_bound() {
    let mut net = Net::new();
    let (c, s, _) = connection(&mut net, 16);
    let r = net.export(s).unwrap();
    net.release(s);
    let chunk = vec![7u8; 4096];
    let mut sent = 0;
    loop {
        match net.send(c, &chunk, None) {
            Ok(n) => sent += n,
            Err(e) => {
                assert_eq!(e, Error::Again, "the peer waits, as for a slow reader");
                break;
            }
        }
    }
    assert_eq!(sent, STREAM_BUF);
    assert_eq!(net.poll(c) & OUT, 0);
    let (s, _) = net.import(&r, 0).unwrap();
    assert_eq!(drain(&mut net, s).unwrap().len(), STREAM_BUF);
    assert_eq!(net.poll(c) & OUT, OUT);
}

#[test]
fn a_close_while_parked_is_seen_after() {
    let mut net = Net::new();
    let (c, s, _) = connection(&mut net, 17);
    let r = net.export(s).unwrap();
    net.release(s);
    net.send(c, b"last", None).unwrap();
    net.release(c);
    let (s, _) = net.import(&r, 0).unwrap();
    assert_eq!(drain(&mut net, s).unwrap(), b"last");
    assert_eq!(drain(&mut net, s).unwrap(), b"", "end of stream");
    assert_eq!(net.send(s, b"x", None), Err(Error::Pipe));
}

#[test]
fn both_ends_of_a_pair_park_together() {
    let mut net = Net::new();
    let (a, b) = net.pair().unwrap();
    net.send(a, b"to b", None).unwrap();
    let (ra, rb) = (net.export(a).unwrap(), net.export(b).unwrap());
    net.release(a);
    net.release(b);
    assert_eq!(net.open(), 2, "parked, not closed");
    let ((a, _), (b, _)) = (net.import(&ra, 0).unwrap(), net.import(&rb, 0).unwrap());
    assert_eq!(drain(&mut net, b).unwrap(), b"to b");
    net.send(b, b"to a", None).unwrap();
    assert_eq!(drain(&mut net, a).unwrap(), b"to a");
}

#[test]
fn unparking_closes_the_connection() {
    let mut net = Net::new();
    let (c, s, l) = connection(&mut net, 18);
    let r = net.export(s).unwrap();
    net.release(s);
    net.unpark(&r);
    assert_eq!(drain(&mut net, c).unwrap(), b"", "the peer sees the end");
    net.unpark(&r); // already gone: nothing
    let (h, _) = net.import(&r, 0).unwrap();
    assert_reset_sequence(&mut net, h);
    for h in [c, l, h] {
        net.release(h);
    }
    assert_eq!(net.open(), 0);
}

#[test]
fn imported_unix_names_are_renumbered_unless_parked() {
    let mut net = Net::new();
    let name = net.new_name();
    let l = net.socket(Family::Unix, Type::Stream).unwrap();
    net.bind(l, &unix(name)).unwrap();
    net.listen(l, 1).unwrap();
    let r = net.export(l).unwrap();
    net.release(l);
    let (l2, renamed) = net.import(&r, 0).unwrap();
    assert_eq!((l2, renamed), (l, Some((name, name))), "parked: kept");
    let mut other = Net::new();
    other.set_boot(1);
    other.new_name();
    let (l2, renamed) = other.import(&r, 0).unwrap();
    let (old, new) = renamed.unwrap();
    assert_eq!(old, name);
    assert_ne!(new, name);
    let c = other.socket(Family::Unix, Type::Stream).unwrap();
    assert_eq!(other.connect(c, &unix(new)), Ok(()));
    assert!(other.accept(l2).is_ok());
}

#[test]
fn records_round_trip_and_reject_damage() {
    let park = Some(Park {
        boot: u64::MAX,
        key: 1 << 40,
    });
    let records = vec![
        Record {
            family: Family::Inet,
            ty: Type::Stream,
            state: Saved::Listening(16),
            local: Some(inet(LOOPBACK, 80)),
            opts: Options {
                linger: Some(3),
                reuse_addr: true,
                rcv_timeout_ns: 5,
                ..Options::default()
            },
            parked: park,
        },
        Record {
            family: Family::Unix,
            ty: Type::Stream,
            state: Saved::Reset,
            local: Some(unix(9)),
            opts: Options::default(),
            parked: None,
        },
        Record {
            family: Family::Inet,
            ty: Type::Dgram,
            state: Saved::Open,
            local: None,
            opts: Options::default(),
            parked: None,
        },
        Record {
            family: Family::Inet,
            ty: Type::Stream,
            state: Saved::Reset,
            local: None,
            opts: Options::default(),
            parked: park,
        },
    ];
    let mut bytes = Vec::new();
    encode_records(&records, &mut bytes);
    assert_eq!(decode_records(&bytes).as_ref(), Ok(&records));
    assert_eq!(
        decode_records(&bytes[..bytes.len() - 1]),
        Err(DecodeError::Truncated)
    );
    let mut extra = bytes.clone();
    extra.push(0);
    assert_eq!(decode_records(&extra), Err(DecodeError::Truncated));
    let mut bad = bytes.clone();
    bad[4] = 7; // family
    assert_eq!(decode_records(&bad), Err(DecodeError::Corrupt));
    for (damaged, why) in [
        (
            Record {
                local: Some(inet(LOOPBACK, 80)),
                ..records[3].clone()
            },
            "a parked connection has no address",
        ),
        (
            Record {
                parked: park,
                ..records[2].clone()
            },
            "only listeners and connections park",
        ),
    ] {
        let mut b = Vec::new();
        encode_records(&[damaged], &mut b);
        assert_eq!(decode_records(&b), Err(DecodeError::Corrupt), "{why}");
    }
}

#[test]
fn the_number_of_sockets_is_bounded() {
    let mut net = Net::new();
    for _ in 0..SOCKETS_MAX {
        net.socket(Family::Inet, Type::Dgram).unwrap();
    }
    assert_eq!(net.socket(Family::Inet, Type::Dgram), Err(Error::TooMany));
    net.release(0);
    assert!(net.socket(Family::Inet, Type::Dgram).is_ok());
}

proptest! {
    /// Decoding arbitrary bytes never panics.
    #[test]
    fn decoding_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..200)) {
        let _ = decode_records(&bytes);
    }

    /// Bytes sent over a connection arrive whole and in order, however the
    /// sends and receives are cut.
    #[test]
    fn streams_keep_order(cuts in proptest::collection::vec((1usize..300, 1usize..300), 1..40)) {
        let mut net = Net::new();
        let (c, s, _) = connection(&mut net, 20);
        let (mut sent, mut got) = (Vec::new(), Vec::new());
        for (i, &(send, take)) in cuts.iter().enumerate() {
            let chunk: Vec<u8> = (0..send).map(|j| (i * 7 + j) as u8).collect();
            let n = net.send(c, &chunk, None).unwrap();
            sent.extend_from_slice(&chunk[..n]);
            let _ = net.recv(s, take, false, &mut |b| { got.extend_from_slice(b); true });
        }
        got.extend(drain(&mut net, s).unwrap_or_default());
        prop_assert_eq!(got, sent);
    }
}

#[test]
fn each_boot_starts_its_ephemeral_ports_elsewhere() {
    let first = |boot: u64| {
        let mut net = Net::new();
        net.set_boot(boot);
        let s = net.socket(Family::Inet, Type::Stream).unwrap();
        net.listen(s, 1).unwrap(); // an ephemeral port
        match net.local_addr(s) {
            Ok(Some(Addr::Inet { port, .. })) => port,
            other => panic!("{other:?}"),
        }
    };
    let ports: alloc::collections::BTreeSet<u16> = (0..64).map(|i| first(1_000_000 + i)).collect();
    assert!(ports.iter().all(|p| EPHEMERAL.contains(p)));
    assert!(ports.len() > 60, "nearby boots start far apart: {ports:?}");
    assert_eq!(first(42), first(42), "the boot decides");
}

/// A socket bound to an ephemeral port, and that port.
fn ephemeral_port(net: &mut Net) -> (Handle, u16) {
    let s = net.socket(Family::Inet, Type::Stream).unwrap();
    net.bind(s, &inet(LOOPBACK, 0)).unwrap();
    match net.local_addr(s) {
        Ok(Some(Addr::Inet { port, .. })) => (s, port),
        other => panic!("{other:?}"),
    }
}

/// Hostile (finding S25): what other cells bind does not show in a cell's
/// ephemeral ports. Each cell takes them in order from a start of its own
/// (design D2 of `close-cross-cell-leaks`); with one counter for all, the
/// second port would be 101 past the first.
#[test]
fn a_cells_ephemeral_ports_do_not_show_other_cells_binds() {
    let mut net = Net::new();
    net.set_boot(7);
    net.set_secret([0x1234_5678, 0x9abc_def0]);
    let kernel_first = net.next_port;
    net.enter(Ports::new(1));
    let (_, first) = ephemeral_port(&mut net);
    let mine = net.leave().unwrap();
    // Another cell binds 100 ports and closes them again.
    net.enter(Ports::new(2));
    let theirs: Vec<Handle> = (0..100).map(|_| ephemeral_port(&mut net).0).collect();
    net.leave();
    theirs.into_iter().for_each(|h| net.release(h));
    net.enter(mine);
    let (_, second) = ephemeral_port(&mut net);
    net.leave();
    let next = if first == *EPHEMERAL.end() {
        *EPHEMERAL.start()
    } else {
        first + 1
    };
    assert_eq!(second, next, "only my own binds move my ports");
    assert_eq!(net.next_port, kernel_first, "nor the kernel's");
    // Starts are spread over the range, and depend on the secret.
    let starts: alloc::collections::BTreeSet<u16> = (1..=64).map(|k| net.first_port(k)).collect();
    assert!(starts.iter().all(|p| EPHEMERAL.contains(p)));
    assert!(starts.len() > 60, "cells start far apart: {starts:?}");
    let before = net.first_port(1);
    net.set_secret([1, 2]);
    assert_ne!(net.first_port(1), before, "the secret decides");
}

/// A network's ports skip its own ports in use, and only those: a port
/// another network (here the kernel's) holds is free in this one (design D5
/// of `add-cell-hierarchy`).
#[test]
fn networks_skip_only_their_own_ports_in_use() {
    let mut net = Net::new();
    net.enter(Ports::new(9));
    let (_, first) = ephemeral_port(&mut net);
    let held = net.leave().unwrap();
    let start = net.first_port(9);
    assert_eq!(first, start);
    // The kernel holds the ports after it; the cell holds the third.
    let k = net.socket(Family::Inet, Type::Stream).unwrap();
    net.bind(k, &inet(LOOPBACK, start + 1)).unwrap();
    net.enter(held);
    let mine = net.socket(Family::Inet, Type::Stream).unwrap();
    net.bind(mine, &inet(LOOPBACK, start + 2)).unwrap();
    assert_eq!(
        ephemeral_port(&mut net).1,
        start + 1,
        "the kernel's port is free here"
    );
    assert_eq!(ephemeral_port(&mut net).1, start + 3, "its own is skipped");
    net.leave();
}

/// The waiters woken since the last call.
pub(crate) fn woken(net: &mut Net) -> Vec<u32> {
    let mut out = Vec::new();
    net.drain_woken(|k| out.push(k));
    out
}

/// Waiter `key` looks at `h` and blocks, as a call that would wait does.
fn wait_on(net: &mut Net, key: u32, h: Handle) {
    net.end_call(|| None);
    net.poll(h);
    net.end_call(|| Some(key));
}

#[test]
fn a_change_wakes_only_the_waiters_of_its_socket() {
    let mut net = Net::new();
    let (c1, s1, _) = connection(&mut net, 20);
    let (c2, s2, _) = connection(&mut net, 21);
    wait_on(&mut net, 1, s1);
    wait_on(&mut net, 2, s2);
    wait_on(&mut net, 2, s2);
    assert!(!net.has_woken());
    net.send(c1, b"x", None).unwrap();
    net.end_call(|| None);
    assert_eq!(woken(&mut net), [1]);
    net.send(c1, b"y", None).unwrap();
    net.end_call(|| None);
    assert_eq!(
        woken(&mut net),
        [],
        "woken once: it looks again when it runs"
    );
    net.release(c2);
    assert_eq!(woken(&mut net), [2], "once, for the end of the stream");
}

/// A cell that woke its own waiters takes them off the woken ones (its
/// threads run next in its slice); the others stay for the scheduler.
#[test]
fn a_cell_takes_its_own_woken_waiters() {
    let mut net = Net::new();
    let (c1, s1, _) = connection(&mut net, 23);
    let (c2, s2, _) = connection(&mut net, 24);
    wait_on(&mut net, 1, s1);
    wait_on(&mut net, 2, s2);
    net.send(c1, b"x", None).unwrap();
    net.send(c2, b"x", None).unwrap();
    net.end_call(|| None);
    assert!(net.take_woken(1));
    assert!(!net.take_woken(1), "taken once");
    assert!(!net.take_woken(3), "never woken");
    assert_eq!(woken(&mut net), [2]);
}

#[test]
fn a_call_that_does_not_block_joins_no_queue() {
    let mut net = Net::new();
    let (c, s, _) = connection(&mut net, 22);
    net.end_call(|| None);
    assert_eq!(drain(&mut net, s), Err(Error::Again));
    net.end_call(|| None);
    net.send(c, b"x", None).unwrap();
    assert!(!net.has_woken());
}

#[test]
fn accept_waits_for_a_connection_and_a_writer_for_room() {
    let mut net = Net::new();
    let l = listener(&mut net, 23);
    net.end_call(|| None);
    assert_eq!(net.accept(l), Err(Error::Again));
    net.end_call(|| Some(5));
    let c = net.socket(Family::Inet, Type::Stream).unwrap();
    net.connect(c, &inet(LOOPBACK, 23)).unwrap();
    net.end_call(|| None);
    assert_eq!(woken(&mut net), [5], "a connection to accept");
    let s = net.accept(l).unwrap();
    net.end_call(|| None);
    let big = vec![1u8; STREAM_BUF];
    assert_eq!(net.send(c, &big, None), Ok(STREAM_BUF));
    assert_eq!(net.send(c, b"x", None), Err(Error::Again));
    net.end_call(|| Some(6));
    assert_eq!(woken(&mut net), [], "the reader is not waiting");
    net.recv(s, 10, false, &mut |_| true).unwrap();
    assert_eq!(woken(&mut net), [6], "room for the writer");
    wait_on(&mut net, 6, c);
    net.recv(s, 10, false, &mut |_| true).unwrap();
    assert_eq!(woken(&mut net), [], "the queue was not full");
}

#[test]
fn closing_wakes_the_waiters_of_the_socket_and_its_peer() {
    let mut net = Net::new();
    let (c, s, _) = connection(&mut net, 24);
    wait_on(&mut net, 7, s);
    wait_on(&mut net, 8, c);
    net.release(s);
    let mut woken = woken(&mut net);
    woken.sort_unstable();
    assert_eq!(woken, [7, 8]);
}

/// The reference test vector of SipHash-2-4 for an 8-byte message: key
/// bytes 00..0f, message bytes 00..07.
#[test]
fn park_keys_use_siphash_2_4() {
    let key = [0x0706_0504_0302_0100, 0x0f0e_0d0c_0b0a_0908];
    assert_eq!(
        super::siphash24(key, 0x0706_0504_0302_0100),
        0x93f5_f579_9a93_2462
    );
}

/// Finding S15: a forged record that guesses a parked socket's key (the
/// counter they used to be, or a neighbour of a key it saw) does not take
/// the socket; only the record that parked it does.
#[test]
fn a_parked_socket_answers_only_its_own_key() {
    let mut net = Net::new();
    net.set_secret([0x1234_5678, 0x9abc_def0]);
    let (_, s, _) = connection(&mut net, 14);
    let (_, s2, _) = connection(&mut net, 15);
    let (r, r2) = (net.export(s).unwrap(), net.export(s2).unwrap());
    net.release(s);
    net.release(s2);
    let (park, park2) = (r.parked.unwrap(), r2.parked.unwrap());
    let mut guesses: Vec<u64> = (0..64).collect();
    guesses.extend([
        park.key ^ 1,
        park.key.wrapping_add(1),
        park2.key.wrapping_sub(1),
    ]);
    guesses.retain(|&g| g != park.key && g != park2.key);
    for key in guesses {
        let forged = Record {
            parked: Some(Park { key, ..park }),
            ..r.clone()
        };
        let (h, _) = net.import(&forged, 0).unwrap();
        assert_ne!(h, s, "key {key} took the parked socket");
        assert_ne!(h, s2, "key {key} took the parked socket");
        net.release(h);
    }
    assert_eq!(net.import(&r, 0).unwrap().0, s, "its own record");
    assert_eq!(net.import(&r2, 0).unwrap().0, s2);
}

/// The socket a call of the cell with network `key` makes.
fn socket_in(net: &mut Net, key: u32, ty: Type) -> Handle {
    net.enter(Ports::new(key));
    let s = net.socket(Family::Inet, ty).unwrap();
    net.leave();
    s
}

/// Hostile (design D5 of `add-cell-hierarchy`, "Separate networks"): a
/// cell's listener, bound datagram socket and Unix name are not there for
/// its sibling, which may bind the same port and name itself.
#[test]
fn networks_keep_listeners_datagrams_and_names_apart() {
    let mut net = Net::new();
    let l = socket_in(&mut net, 1, Type::Stream);
    net.bind(l, &inet(LOOPBACK, 7300)).unwrap();
    net.listen(l, 8).unwrap();
    let c = socket_in(&mut net, 2, Type::Stream);
    assert_eq!(
        net.connect(c, &inet(LOOPBACK, 7300)),
        Err(Error::ConnRefused)
    );
    let any = socket_in(&mut net, 2, Type::Stream);
    assert_eq!(net.connect(any, &inet(ANY, 7300)), Err(Error::ConnRefused));
    let l2 = socket_in(&mut net, 2, Type::Stream);
    assert_eq!(net.bind(l2, &inet(LOOPBACK, 7300)), Ok(()), "the same port");
    net.listen(l2, 8).unwrap();
    let c2 = socket_in(&mut net, 2, Type::Stream);
    net.connect(c2, &inet(LOOPBACK, 7300)).unwrap();
    assert_eq!(net.accept(l), Err(Error::Again), "nothing reached cell 1");
    assert!(net.accept(l2).is_ok());
    // Datagrams.
    let u1 = socket_in(&mut net, 1, Type::Dgram);
    net.bind(u1, &inet(LOOPBACK, 9000)).unwrap();
    let u2 = socket_in(&mut net, 2, Type::Dgram);
    assert_eq!(net.send(u2, b"hi", Some(&inet(LOOPBACK, 9000))), Ok(2));
    assert_eq!(net.poll(u1) & IN, 0, "not delivered to another network");
    let mine = socket_in(&mut net, 1, Type::Dgram);
    assert_eq!(net.send(mine, b"hi", Some(&inet(LOOPBACK, 9000))), Ok(2));
    assert_eq!(net.poll(u1) & IN, IN, "delivered in its own");
    // Unix names: the same number names nothing in another network.
    let name = net.new_name();
    net.enter(Ports::new(1));
    let ul = net.socket(Family::Unix, Type::Stream).unwrap();
    let uc1 = net.socket(Family::Unix, Type::Stream).unwrap();
    net.leave();
    net.bind(ul, &unix(name)).unwrap();
    net.listen(ul, 1).unwrap();
    net.enter(Ports::new(2));
    let uc = net.socket(Family::Unix, Type::Stream).unwrap();
    net.leave();
    assert_eq!(net.connect(uc, &unix(name)), Err(Error::ConnRefused));
    assert_eq!(net.connect(uc1, &unix(name)), Ok(()));
    assert!(net.accept(ul).is_ok());
}

/// A suspended cell's parked listener and connection come back in the
/// network of the resumed cell, with the connections waiting on the
/// listener; its old network has nothing left.
#[test]
fn networks_of_resumed_cells_hold_their_parked_sockets() {
    let mut net = Net::new();
    let l = socket_in(&mut net, 1, Type::Stream);
    net.bind(l, &inet(LOOPBACK, 80)).unwrap();
    net.listen(l, 8).unwrap();
    let c = socket_in(&mut net, 1, Type::Stream);
    net.connect(c, &inet(LOOPBACK, 80)).unwrap();
    let (rl, rc) = (net.export(l).unwrap(), net.export(c).unwrap());
    net.release(l);
    net.release(c);
    let (l2, _) = net.import(&rl, 7).unwrap();
    let (c2, _) = net.import(&rc, 7).unwrap();
    assert_eq!((l2, c2), (l, c), "the same sockets");
    assert_eq!((net.network(l2), net.network(c2)), (Ok(7), Ok(7)));
    let s = net.accept(l2).unwrap();
    assert_eq!(net.network(s), Ok(7), "the waiting connection moved too");
    net.send(c2, b"on", None).unwrap();
    assert_eq!(drain(&mut net, s).unwrap(), b"on");
    let old = socket_in(&mut net, 1, Type::Stream);
    assert_eq!(
        net.connect(old, &inet(LOOPBACK, 80)),
        Err(Error::ConnRefused)
    );
    let new = socket_in(&mut net, 7, Type::Stream);
    assert_eq!(net.connect(new, &inet(LOOPBACK, 80)), Ok(()));
}

/// What one step of [`networks_never_meet`] does.
#[derive(Debug, Clone)]
enum NetOp {
    Listen(u32, u16),
    Connect(u32, u16),
    BindUdp(u32, u16),
    Send(u32, u16),
    Ephemeral(u32),
}

fn net_op() -> impl Strategy<Value = NetOp> {
    let cell = 1..4u32;
    let port = 7000..7004u16;
    prop_oneof![
        (cell.clone(), port.clone()).prop_map(|(c, p)| NetOp::Listen(c, p)),
        (cell.clone(), port.clone()).prop_map(|(c, p)| NetOp::Connect(c, p)),
        (cell.clone(), port.clone()).prop_map(|(c, p)| NetOp::BindUdp(c, p)),
        (cell.clone(), port).prop_map(|(c, p)| NetOp::Send(c, p)),
        cell.prop_map(NetOp::Ephemeral),
    ]
}

/// Whether datagram socket `h` held a datagram (taken).
fn took_datagram(net: &mut Net, h: Handle) -> bool {
    net.recv(h, 64, false, &mut |_| true).is_ok()
}

proptest! {
    /// Hostile property (design D5 of `add-cell-hierarchy`): whatever three
    /// cells do on the same ports, each sees what it would see alone: a
    /// connect reaches a listener of its own network or is refused, a bind
    /// fails only on its own port, a datagram reaches only its own
    /// network's socket, and its next ephemeral port depends on its own
    /// calls only.
    #[test]
    fn networks_never_meet(ops in proptest::collection::vec(net_op(), 1..60)) {
        use alloc::collections::BTreeMap;
        let mut net = Net::new();
        net.set_secret([3, 4]);
        let mut listening: BTreeMap<(u32, u16), Handle> = BTreeMap::new();
        let mut bound: BTreeMap<(u32, u16), Handle> = BTreeMap::new();
        let mut ports: BTreeMap<u32, Ports> = BTreeMap::new();
        for op in ops {
            let cell = match op {
                NetOp::Listen(c, _) | NetOp::Connect(c, _) | NetOp::BindUdp(c, _)
                | NetOp::Send(c, _) | NetOp::Ephemeral(c) => c,
            };
            let mine = *ports.entry(cell).or_insert(Ports::new(cell));
            net.enter(mine);
            match op {
                NetOp::Listen(_, port) => {
                    let l = net.socket(Family::Inet, Type::Stream).unwrap();
                    let taken = listening.contains_key(&(cell, port));
                    let r = net.bind(l, &inet(LOOPBACK, port));
                    prop_assert_eq!(r.is_err(), taken, "bind {}:{}", cell, port);
                    if r.is_ok() {
                        net.listen(l, 64).unwrap();
                        listening.insert((cell, port), l);
                    }
                }
                NetOp::Connect(_, port) => {
                    let c = net.socket(Family::Inet, Type::Stream).unwrap();
                    let r = net.connect(c, &inet(LOOPBACK, port));
                    match listening.get(&(cell, port)) {
                        Some(&l) => {
                            prop_assert_eq!(r, Ok(()));
                            let s = net.accept(l).unwrap();
                            prop_assert_eq!(net.network(s), Ok(cell));
                        }
                        None => prop_assert_eq!(r, Err(Error::ConnRefused)),
                    }
                    for &l in listening.values() {
                        prop_assert_eq!(net.accept(l), Err(Error::Again), "nobody else's");
                    }
                }
                NetOp::BindUdp(_, port) => {
                    let u = net.socket(Family::Inet, Type::Dgram).unwrap();
                    let taken = bound.contains_key(&(cell, port));
                    prop_assert_eq!(net.bind(u, &inet(LOOPBACK, port)).is_err(), taken);
                    bound.entry((cell, port)).or_insert(u);
                }
                NetOp::Send(_, port) => {
                    let u = net.socket(Family::Inet, Type::Dgram).unwrap();
                    net.send(u, b"d", Some(&inet(LOOPBACK, port))).unwrap();
                    for (&(owner, at), &b) in &bound {
                        let got = took_datagram(&mut net, b);
                        prop_assert_eq!(got, owner == cell && at == port, "{}:{}", owner, at);
                    }
                }
                NetOp::Ephemeral(_) => {
                    // Its own ports in the range are all behind its next
                    // one, the fixed ones below the range.
                    let want = match mine.next {
                        0 => net.first_port(cell),
                        next => next,
                    };
                    prop_assert_eq!(ephemeral_port(&mut net).1, want);
                }
            }
            ports.insert(cell, net.leave().unwrap());
        }
    }
}
