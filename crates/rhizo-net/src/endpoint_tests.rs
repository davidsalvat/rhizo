//! Host tests of channel endpoints as sockets (design D10 of
//! `add-cell-hierarchy`), through the public operations.

use super::*;
use rhizo_cap::limits::{MESSAGE_HANDLES_MAX, QUEUE_MESSAGES_MAX};
use rhizo_cap::{Actor, Limits, Recv, Tree};

const ALL: Rights = Rights::SEND.union(Rights::RECEIVE).union(Rights::CARRY);
const H: usize = MESSAGE_HANDLES_MAX as usize;

/// A host channel: its endpoints.
fn channel(net: &mut Net) -> [u32; 2] {
    let ch = net
        .channels_mut()
        .create(&Tree::new(), Actor::Host, &|_| Limits::ZERO)
        .unwrap();
    Channels::endpoints(ch)
}

/// The next message on `h`, whole, with its handles.
fn take(net: &mut Net, h: Handle) -> Result<(Vec<u8>, Vec<u32>)> {
    let (mut out, mut hs) = (Vec::new(), [0; H]);
    match net.recv_message(h, usize::MAX, &mut hs, &mut |p| {
        out.extend_from_slice(p);
        true
    })? {
        Recv::Message { handles, .. } => Ok((out, hs[..handles].to_vec())),
        _ => Err(Error::ConnReset),
    }
}

/// A message wakes the waiters of the sockets holding its endpoint only,
/// and keeps its boundaries.
#[test]
fn endpoint_messages_wake_only_their_endpoint() {
    let mut net = Net::new();
    let [a, b] = channel(&mut net);
    let [c, _] = channel(&mut net);
    let x = net.endpoint(a, 10, ALL).unwrap();
    let y = net.endpoint(b, 11, ALL).unwrap();
    let other = net.endpoint(c, 12, ALL).unwrap();
    assert_eq!(net.kind(y), Ok((Family::Unix, Type::SeqPacket)));
    assert_eq!(net.endpoint_of(y), Some((b, 11, ALL)));
    assert_eq!(net.poll(y) & events::IN, 0);
    assert_eq!(take(&mut net, y), Err(Error::Again));
    net.end_call(|| Some(1)); // waits on `y`
    assert_eq!(net.poll(other), events::OUT);
    net.end_call(|| Some(2)); // waits on `other`
    net.send_message(x, b"hello", &[]).unwrap();
    net.send_message(x, b"world!", &[]).unwrap();
    let mut woken = Vec::new();
    net.drain_woken(|k| woken.push(k));
    assert_eq!(woken, [1], "only the waiter of the peer endpoint");
    assert_eq!(net.poll(y), events::IN | events::OUT);
    let (mut buf, mut hs) = ([0u8; 3], [0; H]);
    let mut at = 0;
    let got = net.recv_message(y, 3, &mut hs, &mut |p| {
        buf[at..at + p.len()].copy_from_slice(p);
        at += p.len();
        true
    });
    assert_eq!(got, Ok(Recv::Message { len: 5, handles: 0 }));
    assert_eq!(&buf, b"hel", "cut to the buffer");
    assert_eq!(take(&mut net, y), Ok((b"world!".to_vec(), vec![])));
}

/// A full queue refuses with `Again` and a receive wakes the senders; the
/// rights of the grant are kept.
#[test]
fn endpoint_queues_are_bounded_and_rights_kept() {
    let mut net = Net::new();
    let [a, b] = channel(&mut net);
    let x = net.endpoint(a, 1, Rights::SEND).unwrap();
    let y = net.endpoint(b, 2, Rights::RECEIVE).unwrap();
    for _ in 0..QUEUE_MESSAGES_MAX {
        net.send_message(x, b"m", &[]).unwrap();
    }
    assert_eq!(net.send_message(x, b"m", &[]), Err(Error::Again));
    assert_eq!(net.poll(x), 0, "no room");
    net.end_call(|| Some(7));
    take(&mut net, y).unwrap();
    let mut woken = Vec::new();
    net.drain_woken(|k| woken.push(k));
    assert_eq!(woken, [7], "room for the sender");
    assert_eq!(net.poll(x), events::OUT);
    assert_eq!(net.send_message(y, b"back", &[]), Err(Error::Access));
    assert_eq!(take(&mut net, x), Err(Error::Access));
    assert_eq!(
        net.send_message(x, b"", &[y]),
        Err(Error::Access),
        "no carry right"
    );
    let s = net.socket(Family::Unix, Type::SeqPacket).unwrap();
    assert_eq!(net.send_message(s, b"", &[]), Err(Error::NotConn));
    assert_eq!(
        net.socket(Family::Inet, Type::SeqPacket),
        Err(Error::OpNotSupp)
    );
}

/// The endpoint closes with its last socket: the peer reads what is queued
/// then end of file, its sends fail with `EPIPE`, the handles queued on
/// the closed endpoint are released and every grant is handed back.
#[test]
fn endpoint_closes_with_its_last_socket() {
    let mut net = Net::new();
    let [a, b] = channel(&mut net);
    let [t0, t1] = channel(&mut net);
    let x = net.endpoint(a, 1, ALL).unwrap();
    let x2 = net.endpoint(a, 2, ALL).unwrap();
    let y = net.endpoint(b, 3, ALL).unwrap();
    let inner = net.endpoint(t0, 4, ALL).unwrap();
    let far = net.endpoint(t1, 5, ALL).unwrap();
    net.send_message(y, b"for x", &[inner]).unwrap();
    net.send_message(x, b"last", &[]).unwrap();
    assert_eq!(net.end_grants(a).collect::<Vec<_>>(), [2, 1]);
    net.release(x);
    assert_eq!(net.take_dropped(), [1]);
    assert_eq!(net.poll(y) & events::HUP, 0, "x2 still holds it");
    net.end_call(|| Some(9));
    net.release(x2);
    let mut woken = Vec::new();
    net.drain_woken(|k| woken.push(k));
    assert!(woken.contains(&9), "the peer sees the close");
    assert_eq!(net.take_dropped(), [2, 4], "the handle in flight too");
    assert_eq!(
        net.poll(far) & events::HUP,
        events::HUP,
        "t0 closed with it"
    );
    assert_eq!(
        net.poll(y),
        events::IN | events::OUT | events::HUP | events::RDHUP
    );
    assert_eq!(take(&mut net, y), Ok((b"last".to_vec(), vec![])));
    let mut hs = [0; H];
    assert_eq!(
        net.recv_message(y, 8, &mut hs, &mut |_| true),
        Ok(Recv::End)
    );
    assert_eq!(net.send_message(y, b"x", &[]), Err(Error::Pipe));
    net.release(y);
    net.release(far);
    assert_eq!(net.open(), 0);
    assert_eq!(net.take_dropped(), [3, 5]);
    assert_eq!(net.channels().owner(a >> 1), None, "the channels ended");
}

/// Endpoints in flight in each other's queues, which no socket outside a
/// message holds any more, stay until their grants go with their holder
/// (the cell that would have received them ends), then are all released
/// without recursion.
#[test]
fn endpoint_cycles_are_released() {
    let mut net = Net::new();
    let mut hs = Vec::new();
    let mut ends = Vec::new();
    for g in 0..100 {
        let [a, b] = channel(&mut net);
        ends.push((net.endpoint(a, 2 * g, ALL).unwrap(), b));
        hs.push(net.endpoint(b, 2 * g + 1, ALL).unwrap());
    }
    // Each channel's far end travels in the next channel's queue.
    for i in 0..100 {
        let (x, _) = ends[(i + 1) % 100];
        net.send_message(x, b"", &[hs[i]]).unwrap();
    }
    for (x, _) in ends {
        net.release(x);
    }
    assert_eq!(net.take_dropped().len(), 100);
    assert_eq!(net.open(), 100, "the far ends, in flight");
    for g in 0..100 {
        net.grant_gone(2 * g + 1);
    }
    assert_eq!(net.open(), 0, "every socket closed");
    // The first revocation closed a channel, which released the rest in
    // flight: their grants come back as dropped.
    assert_eq!(net.take_dropped().len(), 99);
}

/// A revoked grant's sockets read as closed at once, whatever is queued,
/// and the endpoint closes when no other socket holds it.
#[test]
fn endpoint_revoked_reads_closed() {
    let mut net = Net::new();
    let [a, b] = channel(&mut net);
    let x = net.endpoint(a, 1, ALL).unwrap();
    let y = net.endpoint(b, 2, ALL).unwrap();
    let y2 = net.endpoint(b, 3, ALL).unwrap();
    net.send_message(x, b"queued", &[]).unwrap();
    net.end_call(|| None);
    net.poll(y);
    net.end_call(|| Some(4));
    net.grant_gone(2);
    let mut woken = Vec::new();
    net.drain_woken(|k| woken.push(k));
    assert_eq!(woken, [4]);
    let mut hs = [0; H];
    assert_eq!(
        net.recv_message(y, 8, &mut hs, &mut |_| true),
        Ok(Recv::End)
    );
    assert_eq!(net.send_message(y, b"x", &[]), Err(Error::Pipe));
    assert_eq!(net.endpoint_of(y), None);
    assert_eq!(net.poll(x) & events::HUP, 0, "y2 holds the endpoint");
    assert_eq!(take(&mut net, y2), Ok((b"queued".to_vec(), vec![])));
    net.grant_gone(3);
    assert_eq!(net.poll(x) & events::HUP, events::HUP);
    net.release(y);
    assert_eq!(
        net.take_dropped(),
        [] as [u32; 0],
        "a revoked grant is gone already"
    );
}

/// A copy's endpoint reads as closed (design D13 of `add-cell-hierarchy`),
/// and a snapshot record of one comes back closed.
#[test]
fn endpoint_copies_read_closed() {
    let mut net = Net::new();
    let [a, b] = channel(&mut net);
    let x = net.endpoint(a, 1, ALL).unwrap();
    let y = net.endpoint(b, 2, ALL).unwrap();
    let copy = net.fork(x, 9).unwrap();
    assert_eq!(net.kind(copy), Ok((Family::Unix, Type::SeqPacket)));
    assert_eq!(net.endpoint_of(copy), None);
    assert_eq!(net.send_message(copy, b"x", &[]), Err(Error::Pipe));
    assert_eq!(net.take_error(copy), Ok(None), "no reset reported");
    let r = net.export(y).unwrap();
    assert_eq!(
        (r.ty, r.state, r.parked),
        (Type::SeqPacket, Saved::Reset, None)
    );
    let mut bytes = Vec::new();
    encode_records(core::slice::from_ref(&r), &mut bytes);
    assert_eq!(decode_records(&bytes), Ok(vec![r.clone()]));
    let (back, _) = net.import(&r, 9).unwrap();
    let mut hs = [0; H];
    assert_eq!(
        net.recv_message(back, 8, &mut hs, &mut |_| true),
        Ok(Recv::End)
    );
    // A sequenced-packet record of the Inet family is refused.
    bytes[4] = 0;
    assert_eq!(decode_records(&bytes), Err(DecodeError::Corrupt));
}
