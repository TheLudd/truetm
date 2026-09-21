//! Tag system - dvtm/dwm style tagging
//!
//! Each pane can have multiple tags (like labels).
//! Views filter which panes are visible based on selected tags.
//! A pane appears in a view if it has ANY of the view's tags.

use unicode_width::UnicodeWidthChar;

/// Maximum display width of a tag label in the status bar, ellipsis included.
pub const LABEL_MAX_WIDTH: usize = 12;

/// Display columns a string occupies in a terminal.
pub fn display_width(s: &str) -> usize {
    s.chars().map(|c| UnicodeWidthChar::width(c).unwrap_or(0)).sum()
}

/// Trim a label to `max` display columns, keeping both ends.
///
/// Labels that need trimming are usually worktrees of the same repository:
/// they share a prefix and differ at the tail, so cutting only the end would
/// render them identical.
pub fn trim_label(name: &str, max: usize) -> String {
    if display_width(name) <= max {
        return name.to_string();
    }
    if max == 0 {
        return String::new();
    }
    if max == 1 {
        return "\u{2026}".to_string();
    }

    // One column goes to the ellipsis; the head keeps the odd column.
    let budget = max - 1;
    let tail_budget = budget / 2;
    let head_budget = budget - tail_budget;

    let mut head = String::new();
    let mut head_width = 0;
    for c in name.chars() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if head_width + w > head_budget {
            break;
        }
        head.push(c);
        head_width += w;
    }

    let mut tail: Vec<char> = Vec::new();
    let mut tail_width = 0;
    for c in name.chars().rev() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if tail_width + w > tail_budget {
            break;
        }
        tail.push(c);
        tail_width += w;
    }

    let tail: String = tail.into_iter().rev().collect();
    format!("{}\u{2026}{}", head, tail)
}

/// Bitmask representing a set of tags (supports up to 64 tags)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TagSet(pub u64);

#[allow(dead_code)]
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
    fn test_trim_label_leaves_short_names_alone() {
        assert_eq!(trim_label("master", 12), "master");
        assert_eq!(trim_label("Documents", 12), "Documents");
        assert_eq!(trim_label("exactly12chr", 12), "exactly12chr");
    }

    #[test]
    fn test_trim_label_keeps_both_ends() {
        // 6 head + ellipsis + 5 tail
        assert_eq!(trim_label("aios-app-rebuild", 12), "aios-a\u{2026}build");
    }

    #[test]
    fn test_trim_label_distinguishes_sibling_worktrees() {
        let a = trim_label("aios-app-login-fix", LABEL_MAX_WIDTH);
        let b = trim_label("aios-app-logout-fix", LABEL_MAX_WIDTH);
        assert_ne!(a, b);
    }

    #[test]
    fn test_trim_label_respects_display_width() {
        // Double-width characters count as two columns each.
        let trimmed = trim_label("\u{65e5}\u{672c}\u{8a9e}\u{30c6}\u{30b9}\u{30c8}\u{30c7}\u{30fc}\u{30bf}", 12);
        let width: usize = trimmed
            .chars()
            .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
            .sum();
        assert!(width <= 12, "trimmed to {} columns: {}", width, trimmed);
    }

    #[test]
    fn test_trim_label_tiny_budgets() {
        assert_eq!(trim_label("anything", 0), "");
        assert_eq!(trim_label("anything", 1), "\u{2026}");
        assert_eq!(trim_label("anything", 2), "a\u{2026}");
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
