//! Exclusive CPU sets (design D17 of `add-cell-hierarchy`): CPUs given to
//! one domain, which runs only there and which no other domain runs on.
//! The CPUs fall into classes: each set is one, and the CPUs no set holds
//! (the shared CPUs, the boot CPU always among them) are another. An entry
//! runs only on its class's CPUs, so it is queued only on their queues, and
//! a CPU takes, steals and balances only within its class.

use alloc::vec::Vec;

/// Every CPU of a machine with `cpus` CPUs (at most 64).
fn all(cpus: usize) -> u64 {
    match cpus {
        64.. => u64::MAX,
        n => (1 << n) - 1,
    }
}

/// The classes of the CPUs and the class of every entry of a domain with a
/// set; an entry it does not name runs on the shared CPUs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sets {
    /// Per CPU: the CPUs of its class (bit `n` for CPU `n`).
    peers: Vec<u64>,
    /// The entries of domains with a set, by key, sorted: their set.
    keys: Vec<(u32, u64)>,
    /// The CPUs no set holds.
    shared: u64,
}

impl Sets {
    /// Whether a domain may get `set` on a machine of `cpus` CPUs whose
    /// other domains' sets hold `taken`: at least one CPU, only CPUs that
    /// exist, never the boot CPU (CPU 0, busy with the host's work), none
    /// another set holds, and leaving another CPU shared beside the boot
    /// CPU.
    pub fn fits(cpus: usize, taken: u64, set: u64) -> bool {
        let left = all(cpus) & !1 & !taken & !set;
        set != 0 && set & 1 == 0 && set & !all(cpus) == 0 && set & taken == 0 && left != 0
    }

    /// The sets on `cpus` CPUs of the entries `keys` names (an entry's key
    /// and its domain's set, in any order; equal sets are one domain's);
    /// `None` when no set is left. A set that does not [`fit`](Self::fits)
    /// beside those of lower keys is ignored, its entries on the shared
    /// CPUs: the host never gives one.
    pub fn new(cpus: usize, mut keys: Vec<(u32, u64)>) -> Option<Self> {
        keys.sort_unstable();
        let mut taken: Vec<u64> = Vec::new();
        keys.retain(|&(_, set)| {
            if taken.contains(&set) {
                return true;
            }
            let fits = Self::fits(cpus, taken.iter().fold(0, |t, s| t | s), set);
            if fits {
                taken.push(set);
            }
            fits
        });
        if taken.is_empty() {
            return None;
        }
        let shared = all(cpus) & !taken.iter().fold(0, |t, s| t | s);
        let peers = (0..cpus)
            .map(|c| {
                let set = taken.iter().find(|&&s| s & 1 << c != 0);
                set.copied().unwrap_or(shared)
            })
            .collect();
        Some(Self {
            peers,
            keys,
            shared,
        })
    }

    /// The CPUs the entry known by `key` may run on.
    pub fn allowed(&self, key: u32) -> u64 {
        match self.keys.binary_search_by_key(&key, |e| e.0) {
            Ok(i) => self.keys[i].1,
            Err(_) => self.shared,
        }
    }

    /// The CPUs of `cpu`'s class: those whose queues it takes from.
    pub fn peers(&self, cpu: usize) -> u64 {
        self.peers.get(cpu).copied().unwrap_or(self.shared)
    }

    /// The CPUs no set holds.
    pub fn shared(&self) -> u64 {
        self.shared
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_sets_never_take_the_boot_cpu_absent_cpus_or_each_others() {
        assert!(Sets::fits(4, 0, 0b1100));
        assert!(!Sets::fits(4, 0, 0), "empty");
        assert!(!Sets::fits(4, 0, 0b0011), "the boot CPU");
        assert!(!Sets::fits(4, 0, 0b1_0000), "CPU 4 of 4");
        assert!(!Sets::fits(4, 0b0100, 0b1100), "overlaps another set");
        assert!(Sets::fits(4, 0b0100, 0b1000));
        assert!(!Sets::fits(4, 0, 0b1110), "no CPU shared but the boot CPU");
        assert!(!Sets::fits(4, 0b1000, 0b0110), "the same, with another set");
        assert!(Sets::fits(64, 0, 1 << 63));
        assert!(!Sets::fits(2, 0, 0b10), "two CPUs: nothing to give");
        assert!(!Sets::fits(1, 0, u64::MAX), "one CPU: nothing to give");
    }

    #[test]
    fn exclusive_sets_split_the_cpus_into_classes() {
        let s = Sets::new(6, alloc::vec![(9, 0b1100), (3, 0b1100), (4, 0b1_0000)]).unwrap();
        assert_eq!(s.shared(), 0b10_0011);
        assert_eq!(
            (0..6).map(|c| s.peers(c)).collect::<Vec<_>>(),
            [0b10_0011, 0b10_0011, 0b1100, 0b1100, 0b1_0000, 0b10_0011]
        );
        assert_eq!((s.allowed(3), s.allowed(9)), (0b1100, 0b1100));
        assert_eq!(s.allowed(4), 0b1_0000);
        assert_eq!(s.allowed(5), 0b10_0011, "no set: the shared CPUs");
    }

    #[test]
    fn exclusive_sets_that_do_not_fit_are_ignored() {
        // Key 2's set overlaps key 1's; key 3's holds the boot CPU.
        let s = Sets::new(4, alloc::vec![(2, 0b0110), (1, 0b1100), (3, 0b0001)]).unwrap();
        assert_eq!(s.allowed(1), 0b1100);
        assert_eq!((s.allowed(2), s.allowed(3)), (0b0011, 0b0011));
        assert_eq!(Sets::new(4, alloc::vec![(1, 0b0001)]), None);
        assert_eq!(Sets::new(4, Vec::new()), None);
    }
}
