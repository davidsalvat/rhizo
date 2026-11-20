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

#[test]
fn forks_share_listeners_and_reset_connections() {
    let mut net = Net::new();
    let (c, s, l) = connection(&mut net, 12);
    assert_eq!(net.fork(l), Ok(l), "shared");
    let fresh = net.socket(Family::Inet, Type::Stream).unwrap();
    let copy = net.fork(fresh).unwrap();
    assert_ne!(copy, fresh);
    assert_eq!(net.poll(copy), OUT | HUP);
    let reset = net.fork(s).unwrap();
    assert_eq!(drain(&mut net, reset), Err(Error::ConnReset));
    net.send(c, b"still", None).unwrap();
    assert_eq!(
        drain(&mut net, s).unwrap(),
        b"still",
        "the parent keeps its connection"
    );
    net.release(l);
    let c2 = net.socket(Family::Inet, Type::Stream).unwrap();
    net.connect(c2, &inet(LOOPBACK, 12)).unwrap();
    assert!(
        net.accept(l).is_ok(),
        "the fork's reference keeps it listening"
    );
    net.release(l);
    assert_eq!(
        net.connect(fresh, &inet(LOOPBACK, 12)),
        Err(Error::ConnRefused)
    );
}

#[test]
fn snapshot_records_rebind_listeners_and_reset_connections() {
    let mut net = Net::new();
    let (_, s, l) = connection(&mut net, 13);
    let mut o = net.options(l).unwrap();
    o.no_delay = true;
    net.set_options(l, o).unwrap();
    let (rl, rs) = (net.export(l).unwrap(), net.export(s).unwrap());
    assert_eq!(rl.state, Saved::Listening(8));
    assert_eq!(rs.state, Saved::Reset);
    net.release(l);
    net.release(s);
    let (l2, renamed) = net.import(&rl).unwrap();
    assert_eq!(renamed, None);
    assert!(net.is_listening(l2));
    assert!(net.options(l2).unwrap().no_delay);
    let c = net.socket(Family::Inet, Type::Stream).unwrap();
    net.connect(c, &inet(LOOPBACK, 13)).unwrap();
    assert!(net.accept(l2).is_ok());
    let (s2, _) = net.import(&rs).unwrap();
    assert_eq!(drain(&mut net, s2), Err(Error::ConnReset));
    let (taken, _) = net.import(&rl).unwrap();
    assert_eq!(net.poll(taken) & ERR, ERR, "address taken: reset");
}

#[test]
fn imported_unix_names_are_renumbered() {
    let mut net = Net::new();
    let name = net.new_name();
    let l = net.socket(Family::Unix, Type::Stream).unwrap();
    net.bind(l, &unix(name)).unwrap();
    net.listen(l, 1).unwrap();
    let r = net.export(l).unwrap();
    net.release(l);
    let (l2, renamed) = net.import(&r).unwrap();
    let (old, new) = renamed.unwrap();
    assert_eq!(old, name);
    assert_ne!(new, name);
    let c = net.socket(Family::Unix, Type::Stream).unwrap();
    assert_eq!(net.connect(c, &unix(new)), Ok(()));
    assert!(net.accept(l2).is_ok());
}

#[test]
fn records_round_trip_and_reject_damage() {
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
        },
        Record {
            family: Family::Unix,
            ty: Type::Stream,
            state: Saved::Reset,
            local: Some(unix(9)),
            opts: Options::default(),
        },
        Record {
            family: Family::Inet,
            ty: Type::Dgram,
            state: Saved::Open,
            local: None,
            opts: Options::default(),
        },
    ];
    let mut bytes = Vec::new();
    encode_records(&records, &mut bytes);
    assert_eq!(decode_records(&bytes), Ok(records));
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
