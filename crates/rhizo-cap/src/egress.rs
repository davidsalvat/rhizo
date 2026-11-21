//! Egress policies (design D7 of `add-cell-hierarchy`): which outside
//! destinations a cell's sockets may reach, ordered by the set of
//! (protocol, address, port) tuples they allow.

use crate::{Error, alloc_point, reserve};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// The protocol a rule allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Proto {
    Tcp,
    Udp,
    Any,
}

impl Proto {
    /// Whether every packet `other` allows, this allows.
    fn covers(self, other: Proto) -> bool {
        self == Proto::Any || self == other
    }

    fn meet(self, other: Proto) -> Option<Proto> {
        match (self, other) {
            (Proto::Any, p) | (p, Proto::Any) => Some(p),
            (a, b) if a == b => Some(a),
            _ => None,
        }
    }
}

/// One allowed set of destinations: a protocol, an IPv4 network and an
/// inclusive range of ports. Always normalized: the address has no bit
/// past its prefix, and the range is not empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rule {
    proto: Proto,
    net: u32,
    prefix: u8,
    lo: u16,
    hi: u16,
}

/// The mask of a prefix of `len` bits.
fn mask(len: u8) -> u32 {
    if len == 0 { 0 } else { u32::MAX << (32 - len) }
}

impl Rule {
    /// The rule allowing `proto` to `addr/prefix` on ports `lo..=hi`; `None`
    /// if the prefix is over 32 or the range is empty (`EINVAL`).
    pub fn new(proto: Proto, addr: [u8; 4], prefix: u8, lo: u16, hi: u16) -> Option<Self> {
        (prefix <= 32 && lo <= hi).then(|| Self {
            proto,
            net: u32::from_be_bytes(addr) & mask(prefix),
            prefix,
            lo,
            hi,
        })
    }

    pub fn proto(&self) -> Proto {
        self.proto
    }

    /// The network's address and prefix length.
    pub fn net(&self) -> ([u8; 4], u8) {
        (self.net.to_be_bytes(), self.prefix)
    }

    /// The inclusive range of ports.
    pub fn ports(&self) -> (u16, u16) {
        (self.lo, self.hi)
    }

    /// Whether this rule allows `proto` (TCP or UDP) to `addr`:`port`.
    pub fn allows(&self, proto: Proto, addr: [u8; 4], port: u16) -> bool {
        self.proto.covers(proto)
            && u32::from_be_bytes(addr) & mask(self.prefix) == self.net
            && (self.lo..=self.hi).contains(&port)
    }

    /// Whether every destination `other` allows, this rule allows.
    pub fn contains(&self, other: &Rule) -> bool {
        self.proto.covers(other.proto)
            && self.prefix <= other.prefix
            && other.net & mask(self.prefix) == self.net
            && self.lo <= other.lo
            && other.hi <= self.hi
    }

    /// The destinations both rules allow, if any: two networks meet only
    /// when one holds the other.
    pub fn meet(&self, other: &Rule) -> Option<Rule> {
        let proto = self.proto.meet(other.proto)?;
        let (wide, narrow) = if self.prefix <= other.prefix {
            (self, other)
        } else {
            (other, self)
        };
        if narrow.net & mask(wide.prefix) != wide.net {
            return None;
        }
        let (lo, hi) = (self.lo.max(other.lo), self.hi.min(other.hi));
        (lo <= hi).then_some(Rule {
            proto,
            net: narrow.net,
            prefix: narrow.prefix,
            lo,
            hi,
        })
    }
}

/// A cell's egress policy. `Rules` is never empty (no rule is `None`) and
/// holds at most [`RULES_MAX`](crate::limits::RULES_MAX) rules; it is shared
/// and immutable, so sockets keep a reference to the policy they were made
/// under and narrowing replaces it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Egress {
    /// Nothing outside the cell: the default of every cell.
    #[default]
    None,
    Rules(Arc<[Rule]>),
    /// Everything the deny floor leaves.
    Full,
}

impl Egress {
    /// The policy allowing `rules`: `None` for no rule, `Invalid` past
    /// [`RULES_MAX`](crate::limits::RULES_MAX).
    pub fn rules(rules: &[Rule]) -> Result<Self, Error> {
        if rules.len() > crate::limits::RULES_MAX {
            return Err(Error::Invalid);
        }
        if rules.is_empty() {
            return Ok(Egress::None);
        }
        alloc_point()?;
        Ok(Egress::Rules(Arc::from(rules)))
    }

    /// Whether the policy allows `proto` (TCP or UDP) to `addr`:`port`.
    pub fn allows(&self, proto: Proto, addr: [u8; 4], port: u16) -> bool {
        match self {
            Egress::None => false,
            Egress::Rules(rules) => rules.iter().any(|r| r.allows(proto, addr, port)),
            Egress::Full => true,
        }
    }

    /// Whether this policy allows nothing `other` does not: the order of
    /// design D7. Conservative: a rule covered only by the union of several
    /// rules of `other` is not within it, so narrowing refuses in doubt.
    pub fn within(&self, other: &Egress) -> bool {
        match (self, other) {
            (Egress::None, _) | (_, Egress::Full) => true,
            (_, Egress::None) | (Egress::Full, _) => false,
            (Egress::Rules(a), Egress::Rules(b)) => {
                a.iter().all(|r| b.iter().any(|wide| wide.contains(r)))
            }
        }
    }

    /// The destinations both policies allow, as a policy within both. A
    /// pairwise meet of two long lists that comes to more than
    /// [`RULES_MAX`](crate::limits::RULES_MAX) rules keeps the first ones,
    /// which narrows further and is safe.
    pub fn intersect(&self, other: &Egress) -> Result<Egress, Error> {
        let (a, b) = match (self, other) {
            (Egress::None, _) | (_, Egress::None) => return Ok(Egress::None),
            (Egress::Full, p) | (p, Egress::Full) => return Ok(p.clone()),
            (Egress::Rules(a), Egress::Rules(b)) => (a, b),
        };
        let mut out: Vec<Rule> = Vec::new();
        reserve(&mut out, crate::limits::RULES_MAX)?;
        for r in a.iter() {
            for s in b.iter() {
                let Some(m) = r.meet(s) else { continue };
                if out.len() < crate::limits::RULES_MAX && !out.iter().any(|o| o.contains(&m)) {
                    out.retain(|o| !m.contains(o));
                    out.push(m);
                }
            }
        }
        Egress::rules(&out)
    }
}
