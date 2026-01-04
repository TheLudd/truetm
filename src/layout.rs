//! Layout management - arranging panes in the terminal

use crate::pane::{PaneId, Rect};

/// Configuration for layouts
#[derive(Debug, Clone)]
pub struct LayoutConfig {
    /// Ratio of master pane width (0.0 to 1.0)
    pub master_ratio: f32,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self { master_ratio: 0.55 }
    }
}

/// Trait for layout algorithms
pub trait Layout {
    /// Name of this layout
    fn name(&self) -> &str;

    /// Arrange panes within the given area
    /// Returns a vec of (PaneId, Rect) pairs
    fn arrange(&self, pane_ids: &[PaneId], area: Rect, config: &LayoutConfig) -> Vec<(PaneId, Rect)>;
}

/// Vertical stack layout: master on left, stack on right
pub struct VerticalStack;

impl Layout for VerticalStack {
    fn name(&self) -> &str {
        "vertical"
    }

    fn arrange(&self, pane_ids: &[PaneId], area: Rect, config: &LayoutConfig) -> Vec<(PaneId, Rect)> {
        if pane_ids.is_empty() {
            return vec![];
        }

        // Single pane takes full area
        if pane_ids.len() == 1 {
            return vec![(pane_ids[0], area)];
        }

        let mut result = Vec::with_capacity(pane_ids.len());

        // Reserve 1 column for separator between master and stack
        let usable_width = area.width.saturating_sub(1);

        // Master pane on left
        let master_width = ((usable_width as f32) * config.master_ratio) as u16;
        let master_width = master_width.max(1); // At least 1 column

        result.push((
            pane_ids[0],
            Rect::new(area.x, area.y, master_width, area.height),
        ));

        // Stack panes on right (after separator)
        let stack_x = area.x + master_width + 1; // +1 for separator
        let stack_width = usable_width.saturating_sub(master_width).max(1);
        let stack_count = pane_ids.len() - 1;
        let pane_height = area.height / stack_count as u16;
        let remainder = area.height % stack_count as u16;

        let mut y = area.y;
        for (i, &pane_id) in pane_ids[1..].iter().enumerate() {
            // Distribute remainder pixels to first panes
            let extra = if (i as u16) < remainder { 1 } else { 0 };
            let height = pane_height + extra;

            result.push((pane_id, Rect::new(stack_x, y, stack_width, height.max(1))));
            y += height;
        }

        result
    }
}

/// Monocle/fullscreen layout: only focused pane visible
pub struct Monocle;

impl Layout for Monocle {
    fn name(&self) -> &str {
        "monocle"
    }

    fn arrange(&self, pane_ids: &[PaneId], area: Rect, _config: &LayoutConfig) -> Vec<(PaneId, Rect)> {
        // Only show first pane (the focused one should be rotated to first)
        if pane_ids.is_empty() {
            vec![]
        } else {
            vec![(pane_ids[0], area)]
        }
    }
}

/// Layout manager that cycles through available layouts
pub struct LayoutManager {
    layouts: Vec<Box<dyn Layout>>,
    current: usize,
    pub config: LayoutConfig,
}

impl LayoutManager {
    pub fn new() -> Self {
        Self {
            layouts: vec![Box::new(VerticalStack), Box::new(Monocle)],
            current: 0,
            config: LayoutConfig::default(),
        }
    }

    /// Get the current layout
    pub fn current(&self) -> &dyn Layout {
        self.layouts[self.current].as_ref()
    }

    /// Get the current layout name
    pub fn current_name(&self) -> &str {
        self.current().name()
    }

    /// Switch to the next layout
    pub fn next(&mut self) {
        self.current = (self.current + 1) % self.layouts.len();
    }

    /// Arrange panes using the current layout
    pub fn arrange(&self, pane_ids: &[PaneId], area: Rect) -> Vec<(PaneId, Rect)> {
        self.current().arrange(pane_ids, area, &self.config)
    }

    /// Adjust master ratio
    pub fn adjust_master(&mut self, delta: f32) {
        self.config.master_ratio = (self.config.master_ratio + delta).clamp(0.1, 0.9);
    }
}

impl Default for LayoutManager {
    fn default() -> Self {
        Self::new()
    }
}
