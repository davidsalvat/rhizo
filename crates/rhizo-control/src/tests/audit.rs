//! The audit records and their ring (design D18 of `add-cell-hierarchy`).

use super::*;
use crate::audit::{DETAIL_MAX, Kind, RING_BYTES, Record, Ring};

/// Bytes a record takes in the ring.
fn held(r: &Record) -> usize {
    2 + 41 + r.detail.len()
}

fn all(ring: &mut Ring, now: u64) -> Vec<Record> {
    core::iter::from_fn(|| ring.take(now)).collect()
}

const S: u64 = 1_000_000_000;

#[test]
fn audit_records_are_messages_of_version_4_only() {
    let message = Message::Audit(Record {
        seq: 9,
        ns: 7,
        kind: Kind::Attested,
        cell: 3,
        domain: 1,
        count: 1,
        dropped: 2,
        detail: b"template 4".to_vec(),
    });
    let found = frames(&message.encode(0));
    assert_eq!(Message::decode(&found[0]), Ok(message.clone()));
    let old = host_frames(&message.encode_as(PREVIOUS, 0));
    assert_eq!(Message::decode(&old[0]), Err(Error::UnknownKind));
    let mut bad = message.encode(0);
    bad[HEADER_LEN + 16] = 99; // no such kind of record
    assert_eq!(Message::decode(&frames(&bad)[0]), Err(Error::Malformed));
    let mut short = message.encode(0);
    short.truncate(short.len() - 1);
    short[8] -= 1; // the length says so
    assert_eq!(Message::decode(&frames(&short)[0]), Err(Error::Malformed));
}

#[test]
fn audit_kinds_have_names_and_codes() {
    for kind in Kind::ALL {
        assert_eq!(Kind::parse(kind.as_str()), Some(kind));
        assert_eq!(Kind::from_code(kind as u8), Some(kind));
    }
    assert!(Kind::Egress.is_denial() && Kind::Publish.is_denial());
    assert!(!Kind::Attested.is_denial() && !Kind::Created.is_denial());
    assert_eq!(Kind::parse("nothing"), None);
}

#[test]
fn audit_records_leave_in_order_with_their_fields() {
    let mut ring = Ring::new();
    assert!(!ring.pending(0));
    assert_eq!(ring.take(0), None);
    ring.record(5, Kind::Created, 3, 1, b"run hello");
    ring.record(6, Kind::Tree, 4, 1, b"policy: widens");
    assert!(ring.pending(6));
    let got = all(&mut ring, 6);
    assert_eq!(
        got,
        [
            Record {
                seq: 1,
                ns: 5,
                kind: Kind::Created,
                cell: 3,
                domain: 1,
                count: 1,
                dropped: 0,
                detail: b"run hello".to_vec(),
            },
            Record {
                seq: 2,
                ns: 6,
                kind: Kind::Tree,
                cell: 4,
                domain: 1,
                count: 1,
                dropped: 0,
                detail: b"policy: widens".to_vec(),
            },
        ]
    );
    assert!(!ring.pending(6));
}

#[test]
fn a_long_audit_detail_is_cut() {
    let mut ring = Ring::new();
    ring.record(0, Kind::Moved, 1, 1, &[b'x'; 1000]);
    assert_eq!(ring.take(0).unwrap().detail, [b'x'; DETAIL_MAX]);
}

/// The host away while cells cause 100,000 records: recording returns at
/// once every time, the ring stays within its bound, and the first record
/// after the host is back says how many were dropped, which is also the
/// gap in sequence numbers (spec `control-plane`, host away).
#[test]
fn a_full_audit_ring_drops_and_counts() {
    let mut ring = Ring::new();
    for k in 0..100_000u32 {
        ring.record(u64::from(k), Kind::Created, k, k, b"run idle");
    }
    let kept = all(&mut ring, 100_000);
    let bytes: usize = kept.iter().map(held).sum();
    assert!(bytes <= RING_BYTES && bytes > RING_BYTES - 64, "{bytes}");
    assert!(kept.iter().all(|r| r.dropped == 0));
    assert!(kept.windows(2).all(|w| w[1].seq == w[0].seq + 1));
    assert_eq!(kept[0].seq, 1, "the oldest are kept, the newest dropped");
    ring.record(200_000, Kind::Limits, 1, 1, b"");
    let next = ring.take(200_000).unwrap();
    assert_eq!(next.dropped, 100_000 - kept.len() as u64);
    assert_eq!(next.seq, kept.last().unwrap().seq + next.dropped + 1);
    // Room again: the one after carries no drop.
    ring.record(200_001, Kind::Limits, 1, 1, b"");
    assert_eq!(ring.take(200_001).unwrap().dropped, 0);
}

/// 10,000 egress denials of one cell in one second: one record at once,
/// then one carrying the count of the others and the first destination
/// once the second is over (spec `networking`, denial storm).
#[test]
fn audit_denials_fold_per_cell_and_second() {
    let mut ring = Ring::new();
    for k in 0..10_000u64 {
        let to = format!("tcp 10.0.0.{}:443", k % 250);
        ring.record(5 * S + k * 1000, Kind::Egress, 7, 2, to.as_bytes());
    }
    ring.record(5 * S + 1, Kind::Egress, 8, 2, b"tcp 1.1.1.1:53");
    let first = all(&mut ring, 5 * S + 900_000_000);
    assert_eq!(first.len(), 2, "the first of each cell, at once: {first:?}");
    assert_eq!((first[0].cell, first[0].count), (7, 1));
    assert_eq!((first[1].cell, first[1].count), (8, 1));
    assert!(
        !ring.pending(5 * S + 999_999_999),
        "nothing more that second"
    );
    assert!(ring.pending(6 * S));
    let later = all(&mut ring, 6 * S);
    assert_eq!(later.len(), 1, "cell 8 was denied once: nothing to add");
    assert_eq!((later[0].cell, later[0].count), (7, 9_999));
    assert_eq!(later[0].detail, b"tcp 10.0.0.0:443");
    assert_eq!(later[0].kind, Kind::Egress);
    // The next second starts again.
    ring.record(6 * S, Kind::Egress, 7, 2, b"tcp 9.9.9.9:443");
    assert_eq!(ring.take(6 * S).unwrap().count, 1);
}

#[test]
fn audit_folds_keep_kinds_apart() {
    let mut ring = Ring::new();
    for _ in 0..3 {
        ring.record(0, Kind::Egress, 7, 7, b"e");
        ring.record(0, Kind::Tree, 7, 7, b"t");
    }
    let got = all(&mut ring, S);
    let count = |k| -> u32 { got.iter().filter(|r| r.kind == k).map(|r| r.count).sum() };
    assert_eq!((count(Kind::Egress), count(Kind::Tree)), (3, 3));
}

#[test]
fn administrative_audit_records_never_fold() {
    let mut ring = Ring::new();
    for _ in 0..5 {
        ring.record(1, Kind::Policy, 7, 7, b"egress none");
    }
    assert_eq!(all(&mut ring, 1).len(), 5);
}

fn kind() -> impl Strategy<Value = Kind> {
    proptest::sample::select(Kind::ALL.to_vec())
}

proptest! {
    #[test]
    fn audit_record_messages_round_trip(
        seq in any::<u64>(), ns in any::<u64>(), kind in kind(),
        ids in any::<[u32; 3]>(), dropped in any::<u64>(), detail in bytes(),
    ) {
        let message = Message::Audit(Record {
            seq, ns, kind, cell: ids[0], domain: ids[1], count: ids[2], dropped, detail,
        });
        prop_assert_eq!(Message::decode(&frames(&message.encode(0))[0]), Ok(message));
    }

    /// Any records and takes, many cells at once: sequence numbers rise,
    /// every gap is the next record's drop count, the ring never holds more
    /// than its bound, and while nothing is dropped the counts of each
    /// cell's records add up to what it was refused or did.
    #[test]
    fn audit_gaps_are_drop_counts_and_counts_add_up(
        steps in proptest::collection::vec(
            (kind(), 0u32..12, 0usize..300, 0u64..1_500_000_000, any::<bool>()), 1..300),
    ) {
        let mut ring = Ring::new();
        let (mut now, mut last, mut dropped) = (0u64, 0u64, 0u64);
        let mut done = std::collections::HashMap::<(u8, u32), u64>::new();
        let mut seen = std::collections::HashMap::<(u8, u32), u64>::new();
        let mut drain = |ring: &mut Ring, now: u64, seen: &mut std::collections::HashMap<(u8, u32), u64>|
            -> Result<u64, TestCaseError> {
            let mut sum = 0;
            let mut dropped = 0;
            while let Some(r) = ring.take(now) {
                prop_assert_eq!(r.seq, last + r.dropped + 1);
                last = r.seq;
                dropped += r.dropped;
                sum += held(&r);
                *seen.entry((r.kind as u8, r.cell)).or_default() += u64::from(r.count);
            }
            prop_assert!(sum <= RING_BYTES);
            Ok(dropped)
        };
        for (kind, cell, len, dt, take) in steps {
            now += dt;
            ring.record(now, kind, cell, 0, &vec![b'd'; len]);
            *done.entry((kind as u8, cell)).or_default() += 1;
            if take {
                dropped += drain(&mut ring, now, &mut seen)?;
            }
        }
        dropped += drain(&mut ring, now + 2 * S, &mut seen)?;
        if dropped == 0 {
            prop_assert_eq!(seen, done);
        }
    }
}
