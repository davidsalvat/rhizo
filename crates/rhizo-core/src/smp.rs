//! Which CPUs the kernel uses (spec `multi-core`): the boot CPU, then the
//! others the firmware describes, in its order, up to the `smp=` limit and
//! [`MAX_CPUS`].

/// Most CPUs the kernel drives.
pub const MAX_CPUS: usize = 16;

/// The CPUs in use, by firmware identifier (MPIDR affinity on AArch64, APIC
/// identifier on x86_64). Index 0 is the boot CPU; a CPU's index is its
/// number in the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cpus {
    ids: [u64; MAX_CPUS],
    len: usize,
}

impl Cpus {
    /// The CPUs to use: `boot` first (whether or not the firmware lists
    /// it), then the firmware's others in order without duplicates, at most
    /// `limit` (`smp=`) and [`MAX_CPUS`] in all.
    pub fn select(boot: u64, firmware: impl IntoIterator<Item = u64>, limit: Option<u16>) -> Self {
        let max = limit.map_or(MAX_CPUS, |n| (n as usize).clamp(1, MAX_CPUS));
        let mut cpus = Self {
            ids: [0; MAX_CPUS],
            len: 1,
        };
        cpus.ids[0] = boot;
        for id in firmware {
            if cpus.len == max {
                break;
            }
            if !cpus.ids().contains(&id) {
                cpus.ids[cpus.len] = id;
                cpus.len += 1;
            }
        }
        cpus
    }

    /// Firmware identifiers, boot CPU first.
    pub fn ids(&self) -> &[u64] {
        &self.ids[..self.len]
    }

    /// Number of CPUs in use (at least 1).
    pub fn len(&self) -> usize {
        self.len
    }

    /// Always false: the boot CPU is in use.
    pub fn is_empty(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn boot_cpu_first_then_firmware_order() {
        let cpus = Cpus::select(2, [0, 1, 2, 3], None);
        assert_eq!(cpus.ids(), [2, 0, 1, 3]);
        assert_eq!(Cpus::select(5, [], None).ids(), [5], "boot CPU always in");
        assert_eq!(Cpus::select(0, [0, 1, 2, 3], Some(2)).ids(), [0, 1]);
        assert_eq!(Cpus::select(0, [0, 1, 2, 3], Some(1)).ids(), [0], "smp=1");
        assert_eq!(Cpus::select(0, 0..100, None).len(), MAX_CPUS);
        assert_eq!(Cpus::select(0, [1, 1, 0, 1], None).ids(), [0, 1]);
    }

    proptest! {
        /// Whatever the firmware says: the boot CPU first, no duplicates,
        /// every CPU from the firmware or the boot CPU, within the limits,
        /// and no CPU left out while there is room.
        #[test]
        fn selection_invariants(
            boot in 0u64..20,
            firmware in proptest::collection::vec(0u64..20, 0..40),
            limit in proptest::option::of(0u16..20),
        ) {
            let cpus = Cpus::select(boot, firmware.iter().copied(), limit);
            let ids = cpus.ids();
            let max = limit.map_or(MAX_CPUS, |n| (n as usize).clamp(1, MAX_CPUS));
            prop_assert_eq!(ids[0], boot);
            prop_assert!(ids.len() <= max);
            for (i, id) in ids.iter().enumerate() {
                prop_assert!(!ids[..i].contains(id));
                prop_assert!(i == 0 || firmware.contains(id));
            }
            let missing = firmware.iter().filter(|id| !ids.contains(id)).count();
            prop_assert!(missing == 0 || ids.len() == max);
        }
    }
}
