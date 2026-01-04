//! Tag system - dvtm/dwm style tagging
//!
//! Each pane can have multiple tags (like labels).
//! Views filter which panes are visible based on selected tags.
//! A pane appears in a view if it has ANY of the view's tags.

/// Bitmask representing a set of tags (supports up to 64 tags)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TagSet(pub u64);

impl TagSet {
    /// No tags
    pub const NONE: TagSet = TagSet(0);

    /// All tags
    pub const ALL: TagSet = TagSet(u64::MAX);

    /// Create a TagSet with a single tag (0-indexed)
    pub fn single(tag: u8) -> Self {
        debug_assert!(tag < 64);
        TagSet(1 << tag)
    }

    /// Check if this set contains a specific tag
    pub fn contains(&self, tag: u8) -> bool {
        tag < 64 && (self.0 & (1 << tag)) != 0
    }

    /// Add a tag to this set
    pub fn add(&mut self, tag: u8) {
        if tag < 64 {
            self.0 |= 1 << tag;
        }
    }

    /// Remove a tag from this set
    pub fn remove(&mut self, tag: u8) {
        if tag < 64 {
            self.0 &= !(1 << tag);
        }
    }

    /// Toggle a tag in this set
    pub fn toggle(&mut self, tag: u8) {
        if tag < 64 {
            self.0 ^= 1 << tag;
        }
    }

    /// Check if this set intersects with another (any common tags)
    pub fn intersects(&self, other: TagSet) -> bool {
        (self.0 & other.0) != 0
    }

    /// Check if this set is empty
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// Iterator over active tag indices
    pub fn iter(&self) -> impl Iterator<Item = u8> + '_ {
        (0..64).filter(|&i| self.contains(i))
    }

    /// Count of active tags
    pub fn count(&self) -> u32 {
        self.0.count_ones()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single() {
        let t = TagSet::single(0);
        assert!(t.contains(0));
        assert!(!t.contains(1));

        let t = TagSet::single(5);
        assert!(!t.contains(0));
        assert!(t.contains(5));
    }

    #[test]
    fn test_operations() {
        let mut t = TagSet::NONE;
        assert!(t.is_empty());

        t.add(1);
        t.add(3);
        assert!(t.contains(1));
        assert!(t.contains(3));
        assert!(!t.contains(2));

        t.toggle(3);
        assert!(!t.contains(3));

        t.toggle(3);
        assert!(t.contains(3));
    }

    #[test]
    fn test_intersects() {
        let a = TagSet::single(1);
        let b = TagSet::single(2);
        let c = TagSet::single(1);

        assert!(!a.intersects(b));
        assert!(a.intersects(c));

        let mut multi = TagSet::NONE;
        multi.add(1);
        multi.add(2);
        assert!(multi.intersects(a));
        assert!(multi.intersects(b));
    }
}
