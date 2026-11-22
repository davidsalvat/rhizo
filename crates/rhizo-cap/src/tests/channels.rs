//! Channels (design D10): queues, their bounds, and the room reserved
//! from the creator when a channel is made.

use super::{limits, spec};
use crate::limits::{MESSAGE_HANDLES_MAX, MESSAGE_MAX, QUEUE_BYTES_MAX, QUEUE_MESSAGES_MAX};
use crate::*;
use proptest::prelude::*;
use std::collections::VecDeque;

const H: usize = MESSAGE_HANDLES_MAX as usize;

fn none(_: u32) -> Limits {
    Limits::ZERO
}

fn host_channel() -> (Channels, u32, u32) {
    let mut ch = Channels::new();
    let c = ch.create(&Tree::new(), Actor::Host, &none).unwrap();
    let [a, b] = Channels::endpoints(c);
    (ch, a, b)
}

/// Spec `cell-hierarchy`, "Bounded queue": the 65th message to a peer that
/// does not read fails with `EAGAIN`, and the next send after one read
/// succeeds.
#[test]
fn channels_bound_their_queues() {
    let (mut ch, a, b) = host_channel();
    for k in 0..QUEUE_MESSAGES_MAX {
        ch.send(a, &k.to_le_bytes(), &[]).unwrap();
    }
    let before = ch.clone();
    assert_eq!(ch.send(a, b"one more", &[]), Err(Error::Full));
    assert_eq!(ch, before, "a refused send changes nothing");
    let mut buf = [0; 4];
    let mut hs = [0; H];
    assert_eq!(
        ch.recv(b, &mut buf, &mut hs),
        Ok(Recv::Message { len: 4, handles: 0 })
    );
    assert_eq!(buf, 0u32.to_le_bytes(), "oldest first");
    ch.send(a, b"one more", &[]).unwrap();
    // The other way has its own queue.
    ch.send(b, b"back", &[]).unwrap();
    assert_eq!(ch.queued(a), Some((1, 4)));
}

#[test]
fn channels_bound_queued_bytes() {
    let (mut ch, a, b) = host_channel();
    let big = vec![7u8; MESSAGE_MAX as usize];
    for _ in 0..QUEUE_BYTES_MAX / MESSAGE_MAX {
        ch.send(a, &big, &[]).unwrap();
    }
    assert_eq!(ch.send(a, b"x", &[]), Err(Error::Full));
    assert_eq!(
        ch.send(a, &[], &[]),
        Ok(()),
        "an empty message takes no byte"
    );
    assert_eq!(
        ch.send(a, &vec![0; MESSAGE_MAX as usize + 1], &[]),
        Err(Error::TooBig)
    );
    assert_eq!(ch.send(a, b"", &[1; H + 1]), Err(Error::TooManyRefs));
    let mut hs = [0; H];
    let mut buf = vec![0; MESSAGE_MAX as usize];
    ch.recv(b, &mut buf, &mut hs).unwrap();
    assert_eq!(buf, big);
    ch.send(a, b"x", &[]).unwrap();
}

/// Each read takes one whole message: what does not fit the buffer is
/// dropped, its full length reported (`MSG_TRUNC`), and handles come with
/// their message.
#[test]
fn channels_keep_message_boundaries() {
    let (mut ch, a, b) = host_channel();
    ch.send(a, b"hello world", &[3, 4]).unwrap();
    ch.send(a, b"next", &[]).unwrap();
    let mut buf = [0; 5];
    let mut hs = [0; H];
    assert_eq!(
        ch.recv(b, &mut buf, &mut hs),
        Ok(Recv::Message {
            len: 11,
            handles: 2
        })
    );
    assert_eq!((&buf, &hs[..2]), (b"hello", &[3, 4][..]));
    let mut buf = [0; 16];
    assert_eq!(
        ch.recv(b, &mut buf, &mut hs),
        Ok(Recv::Message { len: 4, handles: 0 })
    );
    assert_eq!(&buf[..4], b"next");
    assert_eq!(ch.recv(b, &mut buf, &mut hs), Ok(Recv::Empty));
}

/// A closed peer gives end of file once its messages are read, and
/// `EPIPE` to senders; the channel ends with its second endpoint.
#[test]
fn channels_close_like_sockets() {
    let mut t = Tree::new();
    t.add_root(1, spec(limits(64 << 20, 4)), 0).unwrap();
    let mut ch = Channels::new();
    let c = ch.create(&t, Actor::Cell(1), &none).unwrap();
    let [a, b] = Channels::endpoints(c);
    ch.send(a, b"last", &[]).unwrap();
    ch.send(b, b"lost", &[9]).unwrap();
    let mut dropped = Vec::new();
    assert_eq!(ch.close(a, &mut |h| dropped.push(h)), None);
    assert_eq!(dropped, [9], "handles queued on a closed endpoint");
    assert_eq!(ch.send(b, b"x", &[]), Err(Error::Pipe));
    let (mut buf, mut hs) = ([0; 8], [0; H]);
    assert_eq!(
        ch.recv(b, &mut buf, &mut hs),
        Ok(Recv::Message { len: 4, handles: 0 })
    );
    assert_eq!(ch.recv(b, &mut buf, &mut hs), Ok(Recv::End));
    assert_eq!(ch.send(a, b"x", &[]), Err(Error::NotFound));
    assert_eq!(
        ch.close(b, &mut |_| {}),
        Some(1),
        "the creator's room comes back"
    );
    assert_eq!(ch.owner(c), None);
    // The slot is used again.
    assert_eq!(ch.create(&t, Actor::Host, &none), Ok(c));
}

/// The room of both queues is reserved from the creator's memory budget
/// when the channel is made, and a full queue is refused the same way
/// whatever the budget.
#[test]
fn channels_reserve_room_at_creation() {
    let mut t = Tree::new();
    t.add_root(1, spec(limits(2 * ROOM + ROOM / 2, 4)), 0)
        .unwrap();
    let mut ch = Channels::new();
    let mut made = 0u64;
    loop {
        let usage = |_: u32| Limits {
            memory: made * ROOM,
            ..Limits::ZERO
        };
        match ch.create(&t, Actor::Cell(1), &usage) {
            Ok(_) => made += 1,
            Err(e) => {
                assert_eq!(e, Error::Budget);
                break;
            }
        }
    }
    assert_eq!(made, 2);
    assert_eq!(ch.create(&t, Actor::Cell(9), &none), Err(Error::NotFound));
    assert!(ROOM >= 2 * u64::from(QUEUE_BYTES_MAX));
}

#[derive(Debug, Clone)]
enum Op {
    Send(bool, usize, usize),
    Recv(bool, usize),
    Close(bool),
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 200, ..ProptestConfig::default() })]

    /// Against a model of two queues of whole messages: messages arrive
    /// whole and in order, each queue stays within its bounds, a full queue
    /// refuses with `EAGAIN` and nothing else, and every refusal or failed
    /// allocation changes nothing.
    #[test]
    fn channels_match_their_model(
        ops in prop::collection::vec(prop_oneof![
            6 => (any::<bool>(), 0usize..(MESSAGE_MAX as usize + 2), 0usize..H + 2)
                .prop_map(|(s, n, h)| Op::Send(s, n, h)),
            4 => (any::<bool>(), 0usize..(MESSAGE_MAX as usize + 2)).prop_map(|(s, n)| Op::Recv(s, n)),
            1 => any::<bool>().prop_map(Op::Close),
        ], 1..120),
        fail in prop::option::of(0u32..3),
    ) {
        FAIL_IN.with(|f| f.set(None));
        let (mut ch, a, _) = host_channel();
        let mut model: [VecDeque<(Vec<u8>, Vec<u32>)>; 2] = Default::default();
        let mut open = [true; 2];
        let mut seq = 0u32;
        for op in &ops {
            let before = ch.clone();
            FAIL_IN.with(|f| f.set(fail));
            match *op {
                Op::Send(side, n, h) => {
                    let (s, peer) = (usize::from(side), usize::from(!side));
                    seq += 1;
                    let bytes: Vec<u8> = (0..n).map(|i| (i as u32 ^ seq) as u8).collect();
                    let hs: Vec<u32> = (0..h as u32).map(|i| i + seq).collect();
                    let r = ch.send(a + s as u32, &bytes, &hs);
                    let q = &model[peer];
                    let held: usize = q.iter().map(|m| m.0.len()).sum();
                    let want = if !open[s] {
                        Err(Error::NotFound)
                    } else if n > MESSAGE_MAX as usize {
                        Err(Error::TooBig)
                    } else if h > H {
                        Err(Error::TooManyRefs)
                    } else if !open[peer] {
                        Err(Error::Pipe)
                    } else if q.len() >= QUEUE_MESSAGES_MAX as usize
                        || held + n > QUEUE_BYTES_MAX as usize
                    {
                        Err(Error::Full)
                    } else {
                        Ok(())
                    };
                    if r == Err(Error::NoMemory) && want.is_ok() {
                        prop_assert_eq!(&ch, &before);
                    } else {
                        prop_assert_eq!(r, want);
                        if r.is_ok() {
                            model[peer].push_back((bytes, hs));
                        } else {
                            prop_assert_eq!(&ch, &before);
                        }
                    }
                }
                Op::Recv(side, n) => {
                    let s = usize::from(side);
                    let mut buf = vec![0; n];
                    let mut hs = [0; H];
                    let r = ch.recv(a + s as u32, &mut buf, &mut hs);
                    let want = if !open[s] {
                        Err(Error::NotFound)
                    } else {
                        match model[s].pop_front() {
                            Some((bytes, h)) => {
                                let k = n.min(bytes.len());
                                prop_assert_eq!(&buf[..k], &bytes[..k]);
                                prop_assert_eq!(&hs[..h.len()], &h[..]);
                                Ok(Recv::Message { len: bytes.len(), handles: h.len() })
                            }
                            None if open[1 - s] => Ok(Recv::Empty),
                            None => Ok(Recv::End),
                        }
                    };
                    prop_assert_eq!(r, want);
                }
                Op::Close(side) => {
                    let s = usize::from(side);
                    let mut dropped = Vec::new();
                    let r = ch.close(a + s as u32, &mut |h| dropped.push(h));
                    let want: Vec<u32> = if open[s] {
                        model[s].drain(..).flat_map(|m| m.1).collect()
                    } else {
                        vec![]
                    };
                    prop_assert_eq!(dropped, want);
                    let ended = open[s] && !open[1 - s];
                    prop_assert_eq!(r, ended.then_some(HOST));
                    open[s] = false;
                }
            }
            FAIL_IN.with(|f| f.set(None));
            FAILED.with(|f| f.set(false));
            if !open[0] && !open[1] {
                break;
            }
            for (s, q) in model.iter().enumerate() {
                let (m, b) = ch.queued(a + s as u32).unwrap();
                prop_assert_eq!(m, q.len());
                prop_assert!(m <= QUEUE_MESSAGES_MAX as usize && b <= QUEUE_BYTES_MAX as usize);
            }
        }
    }
}
