//! Which executables stay loaded (design D3 of `add-image-cache`): a few,
//! by image, the least recently used dropped first.

/// An executable: its address and length (images are `'static`, so the
/// pair names one image for as long as the kernel runs).
pub type Key = (u64, u64);

/// Up to `N` loaded spaces (the roots of their hierarchies) by image, in
/// order of use, the most recent last.
#[derive(Debug)]
pub struct Images<const N: usize> {
    slots: [(Key, u64); N],
    len: usize,
}

impl<const N: usize> Images<N> {
    /// No image loaded.
    pub const fn new() -> Self {
        Self {
            slots: [((0, 0), 0); N],
            len: 0,
        }
    }

    /// The space loaded for `key`, which becomes the most recently used.
    pub fn get(&mut self, key: Key) -> Option<u64> {
        let i = self.slots[..self.len].iter().position(|s| s.0 == key)?;
        self.slots[i..self.len].rotate_left(1);
        Some(self.slots[self.len - 1].1)
    }

    /// Adds `root` as the space of `key` (not loaded yet), the most
    /// recently used; returns the space it pushes out when full.
    pub fn insert(&mut self, key: Key, root: u64) -> Option<u64> {
        let out = (self.len == N).then(|| self.remove_at(0));
        self.slots[self.len] = (key, root);
        self.len += 1;
        out
    }

    /// Takes the space of `key` out.
    pub fn remove(&mut self, key: Key) -> Option<u64> {
        let i = self.slots[..self.len].iter().position(|s| s.0 == key)?;
        Some(self.remove_at(i))
    }

    /// The spaces loaded, in no particular order.
    pub fn roots(&self) -> impl Iterator<Item = u64> + '_ {
        self.slots[..self.len].iter().map(|s| s.1)
    }

    /// Takes the least recently used space out.
    pub fn pop(&mut self) -> Option<u64> {
        (self.len > 0).then(|| self.remove_at(0))
    }

    fn remove_at(&mut self, i: usize) -> u64 {
        let root = self.slots[i].1;
        self.slots[i..self.len].rotate_left(1);
        self.len -= 1;
        root
    }
}

impl<const N: usize> Default for Images<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn least_recently_used_goes_first() {
        let mut images = Images::<3>::new();
        assert_eq!(images.get((1, 10)), None);
        for (n, root) in [(1, 0x1000), (2, 0x2000), (3, 0x3000)] {
            assert_eq!(images.insert((n, 10), root), None);
        }
        assert_eq!(images.get((1, 10)), Some(0x1000), "1 is now the newest");
        assert_eq!(images.get((1, 11)), None, "the length is part of the key");
        assert_eq!(images.insert((4, 10), 0x4000), Some(0x2000));
        let mut roots: Vec<u64> = images.roots().collect();
        roots.sort_unstable();
        assert_eq!(roots, [0x1000, 0x3000, 0x4000]);
        assert_eq!(images.get((2, 10)), None);
        assert_eq!(images.remove((3, 10)), Some(0x3000));
        assert_eq!(images.remove((3, 10)), None);
        assert_eq!(images.pop(), Some(0x1000));
        assert_eq!(images.pop(), Some(0x4000));
        assert_eq!(images.pop(), None);
        assert_eq!(images.insert((5, 10), 0x5000), None);
        assert_eq!(images.get((5, 10)), Some(0x5000));
    }
}
