//! The tree section of format version 11 (design D13 and D22 of
//! `add-cell-hierarchy`): a cell's place in the cell tree, its policies and
//! what tells its copies apart.
//!
//! ```text
//! fixed    memory u64, CPU ns u64, sockets u32, descendants u32 (limits);
//!          copy generation u64; CPU set u64 (bit per CPU); number in the
//!          domain u32 (0: unknown); CPU cap u32 (thousandths, u32::MAX:
//!          none); CPU weight u16; budget mode u8 (0: reserved);
//!          on_parent_end u8 (0 cascade, 1 promote); exposed u8 (0, 1);
//!          label length u8; wipe ranges u16; egress kind u8 and rule
//!          count u8; ceiling kind u8 and rule count u8 (kind 0 none, 1
//!          rules, 2 full); 4 zero bytes                    (64 bytes)
//! rules    the egress rules, then the ceiling's: protocol u8 (0 TCP,
//!          1 UDP, 2 any), prefix length u8, first port u16, last port u16,
//!          IPv4 address [4] (no bit past the prefix), 2 zero bytes
//!                                                          (12 bytes each)
//! label    its bytes ([A-Za-z0-9._-])
//! wiped    per `MADV_WIPEONFORK` range: start u64, end u64
//! ```

use alloc::vec::Vec;
use rhizo_cap::limits::{LABEL_MAX, RULES_MAX, WEIGHT_MAX, WEIGHT_MIN};
use rhizo_cap::{Egress, Label, Limits, OnParentEnd, Proto, Rule, Spec};
use rhizo_cell::space::WxRanges;

/// Bytes of the section's fixed part.
pub(crate) const FIXED: usize = 64;
const RULE: usize = 12;

/// The most bytes a section takes, its length included: every rule of an
/// egress policy and a ceiling, the longest label and every wipe range.
pub(crate) const SECTION_MAX: usize =
    8 + FIXED + 2 * RULES_MAX * RULE + LABEL_MAX + rhizo_cell::space::MAX_WX_RANGES * 16;

/// A cell's place in the tree as a snapshot keeps it, with what tells its
/// copies apart (design D22).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeState {
    /// What its creator decided: limits, policy for when its parent ends,
    /// egress and ceiling, label, CPU weight, cap and set.
    pub spec: Spec,
    /// Its number in its domain (0: unknown, a file of version 10).
    pub pid: u32,
    /// Its copy generation.
    pub generation: u64,
    /// Whether it may hold a secret (the sticky bit of design D22).
    pub exposed: bool,
    /// Its `MADV_WIPEONFORK` ranges.
    pub wiped: Vec<(u64, u64)>,
}

fn proto_byte(p: Proto) -> u8 {
    match p {
        Proto::Tcp => 0,
        Proto::Udp => 1,
        Proto::Any => 2,
    }
}

fn rules_of(e: &Egress) -> (u8, &[Rule]) {
    match e {
        Egress::None => (0, &[]),
        Egress::Rules(r) => (1, r),
        Egress::Full => (2, &[]),
    }
}

/// Appends the section for `t` (its length first) to `out`.
pub(crate) fn encode(t: &TreeState, out: &mut Vec<u8>) {
    let s = &t.spec;
    let ceiling = s.ceiling.as_ref().unwrap_or(&s.egress);
    let ((ek, er), (ck, cr)) = (rules_of(&s.egress), rules_of(ceiling));
    let label = s.label.as_bytes();
    let len = FIXED + (er.len() + cr.len()) * RULE + label.len() + t.wiped.len() * 16;
    out.reserve(8 + len);
    out.extend_from_slice(&(len as u64).to_le_bytes());
    let l = s.limits;
    out.extend_from_slice(&l.memory.to_le_bytes());
    out.extend_from_slice(&l.cpu_ns.to_le_bytes());
    out.extend_from_slice(&l.sockets.to_le_bytes());
    out.extend_from_slice(&l.cells.to_le_bytes());
    out.extend_from_slice(&t.generation.to_le_bytes());
    out.extend_from_slice(&s.cpus.to_le_bytes());
    out.extend_from_slice(&t.pid.to_le_bytes());
    out.extend_from_slice(&s.cpu_max.to_le_bytes());
    out.extend_from_slice(&s.weight.to_le_bytes());
    let on_end = (s.on_parent_end == OnParentEnd::Promote) as u8;
    // Counts fit: at most RULES_MAX rules, LABEL_MAX bytes, 64 ranges.
    out.extend_from_slice(&[0, on_end, t.exposed as u8, label.len() as u8]);
    out.extend_from_slice(&(t.wiped.len() as u16).to_le_bytes());
    out.extend_from_slice(&[ek, er.len() as u8, ck, cr.len() as u8, 0, 0, 0, 0]);
    for r in er.iter().chain(cr) {
        let ((addr, prefix), (lo, hi)) = (r.net(), r.ports());
        out.extend_from_slice(&[proto_byte(r.proto()), prefix]);
        out.extend_from_slice(&lo.to_le_bytes());
        out.extend_from_slice(&hi.to_le_bytes());
        out.extend_from_slice(&addr);
        out.extend_from_slice(&[0, 0]);
    }
    out.extend_from_slice(label);
    for &(start, end) in &t.wiped {
        out.extend_from_slice(&start.to_le_bytes());
        out.extend_from_slice(&end.to_le_bytes());
    }
}

/// What every resume needs of a section `b` already validated (contents
/// without its length), read in place: the cell's number, copy generation
/// and sticky bit.
pub(crate) fn copy_state(b: &[u8]) -> (u32, u64, bool) {
    match (b.get(40..44), b.get(24..32), b.get(52)) {
        (Some(pid), Some(generation), Some(&exposed)) => (
            u32::from_le_bytes(pid.try_into().unwrap_or([0; 4])),
            u64::from_le_bytes(generation.try_into().unwrap_or([0; 8])),
            exposed == 1,
        ),
        _ => (0, crate::UNKNOWN_GENERATION, true),
    }
}

/// The wipe ranges of a section `b` already validated, read in place
/// (nothing allocated when there are none, as most often).
pub(crate) fn wiped(b: &[u8]) -> Vec<(u64, u64)> {
    let count = b
        .get(54..56)
        .map_or(0, |c| u16::from_le_bytes([c[0], c[1]]) as usize);
    if count == 0 {
        return Vec::new();
    }
    let u64_at = |at: usize| {
        b.get(at..at + 8)
            .map_or(0, |w| u64::from_le_bytes(w.try_into().unwrap_or([0; 8])))
    };
    let at = FIXED + (b[57] as usize + b[59] as usize) * RULE + b[53] as usize;
    (0..count)
        .map(|i| (u64_at(at + i * 16), u64_at(at + i * 16 + 8)))
        .collect()
}

/// Decodes the policy of `kind` with `rules` encoded in `b`; `None` if
/// anything in it is malformed.
fn decode_egress(kind: u8, b: &[u8]) -> Option<Egress> {
    let mut rules = Vec::new();
    for r in b.chunks_exact(RULE) {
        let proto = match r[0] {
            0 => Proto::Tcp,
            1 => Proto::Udp,
            2 => Proto::Any,
            _ => return None,
        };
        let (lo, hi) = (
            u16::from_le_bytes([r[2], r[3]]),
            u16::from_le_bytes([r[4], r[5]]),
        );
        let addr = [r[6], r[7], r[8], r[9]];
        let rule = Rule::new(proto, addr, r[1], lo, hi)?;
        // One encoding per rule: no bit past the prefix, zero padding.
        if rule.net().0 != addr || r[10..] != [0, 0] {
            return None;
        }
        rules.push(rule);
    }
    match (kind, rules.len()) {
        (0, 0) => Some(Egress::None),
        (2, 0) => Some(Egress::Full),
        (1, 1..=RULES_MAX) => Egress::rules(&rules).ok(),
        _ => None,
    }
}

/// Decodes a section's contents (without its length); `None` if anything
/// in it is malformed: the bytes are untrusted.
pub(crate) fn decode(b: &[u8]) -> Option<TreeState> {
    if b.len() < FIXED {
        return None;
    }
    let u64_at = |at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap_or([0; 8]));
    let u32_at = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap_or([0; 4]));
    let weight = u16::from_le_bytes([b[48], b[49]]);
    let (mode, on_end, exposed, label_len) = (b[50], b[51], b[52], b[53] as usize);
    let wiped = u16::from_le_bytes([b[54], b[55]]) as usize;
    let (ek, en, ck, cn) = (b[56], b[57] as usize, b[58], b[59] as usize);
    let len = FIXED + (en + cn) * RULE + label_len + wiped * 16;
    if b.len() != len
        || mode != 0
        || on_end > 1
        || exposed > 1
        || !(WEIGHT_MIN..=WEIGHT_MAX).contains(&weight)
        || label_len > LABEL_MAX
        || b[60..64] != [0; 4]
    {
        return None;
    }
    let mut at = FIXED;
    let egress = decode_egress(ek, &b[at..at + en * RULE])?;
    at += en * RULE;
    let ceiling = decode_egress(ck, &b[at..at + cn * RULE])?;
    at += cn * RULE;
    let label = Label::new(&b[at..at + label_len]).ok()?;
    at += label_len;
    let ranges: Vec<(u64, u64)> = (0..wiped)
        .map(|i| (u64_at(at + i * 16), u64_at(at + i * 16 + 8)))
        .collect();
    WxRanges::default().with_wiped(ranges.clone())?;
    Some(TreeState {
        spec: Spec {
            limits: Limits {
                memory: u64_at(0),
                cpu_ns: u64_at(8),
                sockets: u32_at(16),
                cells: u32_at(20),
            },
            on_parent_end: if on_end == 1 {
                OnParentEnd::Promote
            } else {
                OnParentEnd::Cascade
            },
            egress,
            ceiling: Some(ceiling),
            label,
            weight,
            cpu_max: u32_at(44),
            cpus: u64_at(32),
        },
        pid: u32_at(40),
        generation: u64_at(24),
        exposed: exposed == 1,
        wiped: ranges,
    })
}
