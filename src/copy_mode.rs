//! Vim-style copy mode for scrollback navigation and text selection

/// Character classification for word motions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharClass {
    Whitespace,
    Word,  // alphanumeric + underscore
    Punct, // everything else
}

impl CharClass {
    pub fn of(c: char) -> Self {
        if c.is_whitespace() {
            CharClass::Whitespace
        } else if c.is_alphanumeric() || c == '_' {
            CharClass::Word
        } else {
            CharClass::Punct
        }
    }

    /// For WORD motions: only whitespace vs non-whitespace
    pub fn of_word(c: char) -> Self {
        if c.is_whitespace() {
            CharClass::Whitespace
        } else {
            CharClass::Word
        }
    }
}

/// Position in the buffer (x is column, y can be negative for scrollback)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferPos {
    pub x: u16,
    pub y: i32, // Negative = scrollback lines
}

impl BufferPos {
    pub fn new(x: u16, y: i32) -> Self {
        Self { x, y }
    }
}

/// Visual selection mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualMode {
    None,
    Char, // v - character-wise selection
    Line, // V - line-wise selection
}

/// Selection anchor and cursor
#[derive(Debug, Clone, Copy)]
pub struct Selection {
    pub anchor: BufferPos,
    pub cursor: BufferPos,
}

impl Selection {
    pub fn new(pos: BufferPos) -> Self {
        Self {
            anchor: pos,
            cursor: pos,
        }
    }

    /// Get normalized selection bounds (start <= end)
    pub fn bounds(&self) -> (BufferPos, BufferPos) {
        if self.anchor.y < self.cursor.y
            || (self.anchor.y == self.cursor.y && self.anchor.x <= self.cursor.x)
        {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }
}

/// Copy mode state
pub struct CopyModeState {
    pub cursor: BufferPos,
    pub scroll_offset: usize, // Lines scrolled into history
    pub visual_mode: VisualMode,
    pub selection: Option<Selection>,
    pub buffer_height: u16,   // Visible buffer height
    pub buffer_width: u16,    // Buffer width
    pub scrollback_len: usize, // Total scrollback lines available
}

impl CopyModeState {
    pub fn new(buffer_width: u16, buffer_height: u16, scrollback_len: usize) -> Self {
        Self {
            // Start cursor at bottom-left of visible area
            cursor: BufferPos::new(0, buffer_height.saturating_sub(1) as i32),
            scroll_offset: 0,
            visual_mode: VisualMode::None,
            selection: None,
            buffer_height,
            buffer_width,
            scrollback_len,
        }
    }

    /// Update buffer dimensions (on resize)
    pub fn update_dimensions(&mut self, width: u16, height: u16, scrollback_len: usize) {
        self.buffer_width = width;
        self.buffer_height = height;
        self.scrollback_len = scrollback_len;
        // Clamp cursor to new bounds
        self.clamp_cursor();
    }

    /// Clamp cursor position to valid bounds
    fn clamp_cursor(&mut self) {
        let max_y = (self.buffer_height as i32) - 1;
        let min_y = -(self.scrollback_len as i32);
        self.cursor.y = self.cursor.y.clamp(min_y, max_y);
        self.cursor.x = self.cursor.x.min(self.buffer_width.saturating_sub(1));
    }

    /// Move cursor, updating selection if in visual mode
    fn move_cursor(&mut self, new_pos: BufferPos) {
        self.cursor = new_pos;
        self.clamp_cursor();

        // Update selection if in visual mode
        if self.visual_mode != VisualMode::None {
            if let Some(ref mut sel) = self.selection {
                sel.cursor = self.cursor;
            }
        }

        // Ensure cursor is visible by adjusting scroll
        self.ensure_cursor_visible();
    }

    /// Ensure cursor is visible in the viewport
    fn ensure_cursor_visible(&mut self) {
        // Convert cursor y to scroll-relative position
        // cursor.y = 0 is top of visible buffer when scroll_offset = 0
        // cursor.y = -1 is first scrollback line
        // With scroll_offset = N, visible range is -N to (buffer_height - 1 - N)

        let visible_top = -(self.scroll_offset as i32);
        let visible_bottom = (self.buffer_height as i32) - 1 - (self.scroll_offset as i32);

        if self.cursor.y < visible_top {
            // Cursor above visible area - scroll up
            self.scroll_offset = (-self.cursor.y) as usize;
        } else if self.cursor.y > visible_bottom {
            // Cursor below visible area - scroll down
            let needed_offset = self.cursor.y - (self.buffer_height as i32 - 1);
            self.scroll_offset = (-needed_offset).max(0) as usize;
        }

        // Clamp scroll offset
        self.scroll_offset = self.scroll_offset.min(self.scrollback_len);
    }

    // === Basic Motions ===

    /// Move left (h)
    pub fn move_left(&mut self) {
        let new_x = self.cursor.x.saturating_sub(1);
        self.move_cursor(BufferPos::new(new_x, self.cursor.y));
    }

    /// Move right (l)
    pub fn move_right(&mut self) {
        let new_x = (self.cursor.x + 1).min(self.buffer_width.saturating_sub(1));
        self.move_cursor(BufferPos::new(new_x, self.cursor.y));
    }

    /// Move up (k)
    pub fn move_up(&mut self) {
        let min_y = -(self.scrollback_len as i32);
        let new_y = (self.cursor.y - 1).max(min_y);
        self.move_cursor(BufferPos::new(self.cursor.x, new_y));
    }

    /// Move down (j)
    pub fn move_down(&mut self) {
        let max_y = (self.buffer_height as i32) - 1;
        let new_y = (self.cursor.y + 1).min(max_y);
        self.move_cursor(BufferPos::new(self.cursor.x, new_y));
    }

    /// Move to start of line (0)
    pub fn move_to_line_start(&mut self) {
        self.move_cursor(BufferPos::new(0, self.cursor.y));
    }

    /// Move to end of line ($) - moves to last non-space character
    pub fn move_to_line_end(&mut self, line_content: &[char]) {
        // Find last non-space character
        let end_x = line_content
            .iter()
            .rposition(|&c| c != ' ')
            .unwrap_or(0) as u16;
        self.move_cursor(BufferPos::new(end_x, self.cursor.y));
    }

    /// Move to first non-blank character (^)
    pub fn move_to_first_non_blank(&mut self, line_content: &[char]) {
        let first_non_blank = line_content
            .iter()
            .position(|&c| !c.is_whitespace())
            .unwrap_or(0) as u16;
        self.move_cursor(BufferPos::new(first_non_blank, self.cursor.y));
    }

    /// Move to top of buffer (gg)
    pub fn move_to_top(&mut self) {
        let min_y = -(self.scrollback_len as i32);
        self.move_cursor(BufferPos::new(0, min_y));
    }

    /// Move to bottom of buffer (G)
    pub fn move_to_bottom(&mut self) {
        let max_y = (self.buffer_height as i32) - 1;
        self.move_cursor(BufferPos::new(0, max_y));
    }

    /// Move to top of visible screen (H)
    pub fn move_to_screen_top(&mut self) {
        let visible_top = -(self.scroll_offset as i32);
        self.move_cursor(BufferPos::new(self.cursor.x, visible_top));
    }

    /// Move to middle of visible screen (M)
    pub fn move_to_screen_middle(&mut self) {
        let visible_top = -(self.scroll_offset as i32);
        let middle = visible_top + (self.buffer_height as i32 / 2);
        self.move_cursor(BufferPos::new(self.cursor.x, middle));
    }

    /// Move to bottom of visible screen (L)
    pub fn move_to_screen_bottom(&mut self) {
        let visible_bottom = (self.buffer_height as i32) - 1 - (self.scroll_offset as i32);
        self.move_cursor(BufferPos::new(self.cursor.x, visible_bottom));
    }

    /// Page up (Ctrl+U or PgUp)
    pub fn page_up(&mut self) {
        let half_page = (self.buffer_height / 2).max(1) as i32;
        let min_y = -(self.scrollback_len as i32);
        let new_y = (self.cursor.y - half_page).max(min_y);
        self.move_cursor(BufferPos::new(self.cursor.x, new_y));
    }

    /// Page down (Ctrl+D or PgDown)
    pub fn page_down(&mut self) {
        let half_page = (self.buffer_height / 2).max(1) as i32;
        let max_y = (self.buffer_height as i32) - 1;
        let new_y = (self.cursor.y + half_page).min(max_y);
        self.move_cursor(BufferPos::new(self.cursor.x, new_y));
    }

    // === Visual Mode ===

    /// Toggle character-wise visual mode (v)
    pub fn toggle_visual_char(&mut self) {
        if self.visual_mode == VisualMode::Char {
            self.visual_mode = VisualMode::None;
            self.selection = None;
        } else {
            self.visual_mode = VisualMode::Char;
            self.selection = Some(Selection::new(self.cursor));
        }
    }

    /// Toggle line-wise visual mode (V)
    pub fn toggle_visual_line(&mut self) {
        if self.visual_mode == VisualMode::Line {
            self.visual_mode = VisualMode::None;
            self.selection = None;
        } else {
            self.visual_mode = VisualMode::Line;
            self.selection = Some(Selection::new(self.cursor));
        }
    }

    /// Get the current selection bounds for rendering, accounting for visual mode
    /// Returns (start_x, start_y, end_x, end_y) in buffer coordinates
    pub fn get_selection_bounds(&self) -> Option<(u16, i32, u16, i32)> {
        let sel = self.selection.as_ref()?;
        let (start, end) = sel.bounds();

        match self.visual_mode {
            VisualMode::None => None,
            VisualMode::Char => Some((start.x, start.y, end.x, end.y)),
            VisualMode::Line => {
                // Line mode selects entire lines
                Some((0, start.y, self.buffer_width.saturating_sub(1), end.y))
            }
        }
    }

    /// Check if a cell is selected (for rendering)
    pub fn is_selected(&self, x: u16, y: i32) -> bool {
        if let Some((start_x, start_y, end_x, end_y)) = self.get_selection_bounds() {
            if y < start_y || y > end_y {
                return false;
            }
            if y == start_y && y == end_y {
                // Single line selection
                return x >= start_x && x <= end_x;
            }
            if y == start_y {
                return x >= start_x;
            }
            if y == end_y {
                return x <= end_x;
            }
            // Middle lines are fully selected
            true
        } else {
            false
        }
    }

    /// Convert screen Y coordinate to buffer Y coordinate
    pub fn screen_y_to_buffer_y(&self, screen_y: u16) -> i32 {
        (screen_y as i32) - (self.scroll_offset as i32)
    }

    /// Convert buffer Y coordinate to screen Y coordinate (None if not visible)
    pub fn buffer_y_to_screen_y(&self, buffer_y: i32) -> Option<u16> {
        let screen_y = buffer_y + (self.scroll_offset as i32);
        if screen_y >= 0 && screen_y < self.buffer_height as i32 {
            Some(screen_y as u16)
        } else {
            None
        }
    }

    /// Get cursor position in screen coordinates (None if not visible)
    pub fn cursor_screen_pos(&self) -> Option<(u16, u16)> {
        self.buffer_y_to_screen_y(self.cursor.y)
            .map(|y| (self.cursor.x, y))
    }

    // === Word Motions ===

    /// Move to start of next word (w)
    pub fn move_word_forward(&mut self, line_content: &[char], big_word: bool) {
        let classify = if big_word { CharClass::of_word } else { CharClass::of };
        let x = self.cursor.x as usize;
        let len = line_content.len();

        if x >= len {
            return;
        }

        let current_class = classify(line_content[x]);
        let mut pos = x;

        // Skip current word (same class)
        while pos < len && classify(line_content[pos]) == current_class {
            pos += 1;
        }

        // Skip whitespace
        while pos < len && classify(line_content[pos]) == CharClass::Whitespace {
            pos += 1;
        }

        if pos < len {
            self.move_cursor(BufferPos::new(pos as u16, self.cursor.y));
        } else {
            // End of line - stay at last position
            self.move_cursor(BufferPos::new((len.saturating_sub(1)) as u16, self.cursor.y));
        }
    }

    /// Move to start of previous word (b)
    pub fn move_word_backward(&mut self, line_content: &[char], big_word: bool) {
        let classify = if big_word { CharClass::of_word } else { CharClass::of };
        let x = self.cursor.x as usize;

        if x == 0 || line_content.is_empty() {
            return;
        }

        let mut pos = x.saturating_sub(1);

        // Skip whitespace backwards
        while pos > 0 && classify(line_content[pos]) == CharClass::Whitespace {
            pos -= 1;
        }

        if pos == 0 {
            self.move_cursor(BufferPos::new(0, self.cursor.y));
            return;
        }

        let target_class = classify(line_content[pos]);

        // Skip to start of current word
        while pos > 0 && classify(line_content[pos - 1]) == target_class {
            pos -= 1;
        }

        self.move_cursor(BufferPos::new(pos as u16, self.cursor.y));
    }

    /// Move to end of word (e)
    pub fn move_word_end(&mut self, line_content: &[char], big_word: bool) {
        let classify = if big_word { CharClass::of_word } else { CharClass::of };
        let x = self.cursor.x as usize;
        let len = line_content.len();

        if x >= len.saturating_sub(1) {
            return;
        }

        let mut pos = x + 1;

        // Skip whitespace
        while pos < len && classify(line_content[pos]) == CharClass::Whitespace {
            pos += 1;
        }

        if pos >= len {
            return;
        }

        let target_class = classify(line_content[pos]);

        // Move to end of word
        while pos + 1 < len && classify(line_content[pos + 1]) == target_class {
            pos += 1;
        }

        self.move_cursor(BufferPos::new(pos as u16, self.cursor.y));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_movement() {
        let mut state = CopyModeState::new(80, 24, 100);

        // Start at bottom-left
        assert_eq!(state.cursor.x, 0);
        assert_eq!(state.cursor.y, 23);

        // Move right
        state.move_right();
        assert_eq!(state.cursor.x, 1);

        // Move left
        state.move_left();
        assert_eq!(state.cursor.x, 0);

        // Move up
        state.move_up();
        assert_eq!(state.cursor.y, 22);

        // Move down
        state.move_down();
        assert_eq!(state.cursor.y, 23);
    }

    #[test]
    fn test_line_navigation() {
        let mut state = CopyModeState::new(80, 24, 100);
        state.cursor.x = 10;

        // Move to start
        state.move_to_line_start();
        assert_eq!(state.cursor.x, 0);

        // Move to end (with line content "hello     " padded to 80)
        let mut line: Vec<char> = "hello".chars().collect();
        line.resize(80, ' ');
        state.move_to_line_end(&line);
        assert_eq!(state.cursor.x, 4); // last char of "hello" is at index 4
    }

    #[test]
    fn test_scrollback_navigation() {
        let mut state = CopyModeState::new(80, 24, 100);

        // Go to top (into scrollback)
        state.move_to_top();
        assert_eq!(state.cursor.y, -100);

        // Go to bottom
        state.move_to_bottom();
        assert_eq!(state.cursor.y, 23);
    }

    #[test]
    fn test_visual_char_selection() {
        let mut state = CopyModeState::new(80, 24, 100);
        state.cursor = BufferPos::new(5, 10);

        // Enter visual mode
        state.toggle_visual_char();
        assert_eq!(state.visual_mode, VisualMode::Char);
        assert!(state.selection.is_some());

        // Move cursor
        state.move_right();
        state.move_right();
        state.move_down();

        // Check selection bounds
        let bounds = state.get_selection_bounds().unwrap();
        assert_eq!(bounds, (5, 10, 7, 11));

        // Exit visual mode
        state.toggle_visual_char();
        assert_eq!(state.visual_mode, VisualMode::None);
        assert!(state.selection.is_none());
    }

    #[test]
    fn test_visual_line_selection() {
        let mut state = CopyModeState::new(80, 24, 100);
        state.cursor = BufferPos::new(5, 10);

        // Enter line visual mode
        state.toggle_visual_line();
        assert_eq!(state.visual_mode, VisualMode::Line);

        // Move down
        state.move_down();
        state.move_down();

        // Check selection bounds (full lines)
        let bounds = state.get_selection_bounds().unwrap();
        assert_eq!(bounds, (0, 10, 79, 12));
    }

    #[test]
    fn test_word_forward() {
        let mut state = CopyModeState::new(80, 24, 100);
        state.cursor = BufferPos::new(0, 0);

        // "hello world_test  foo.bar"
        let line: Vec<char> = "hello world_test  foo.bar".chars().collect();

        // w from start -> "world"
        state.move_word_forward(&line, false);
        assert_eq!(state.cursor.x, 6); // 'w' of world

        // w again -> "foo"
        state.move_word_forward(&line, false);
        assert_eq!(state.cursor.x, 18); // 'f' of foo

        // w again -> "." (punct is separate word class)
        state.cursor.x = 18;
        state.move_word_forward(&line, false);
        assert_eq!(state.cursor.x, 21); // '.'

        // W (big word) from start
        state.cursor.x = 0;
        state.move_word_forward(&line, true);
        assert_eq!(state.cursor.x, 6); // 'w' of world_test

        state.move_word_forward(&line, true);
        assert_eq!(state.cursor.x, 18); // 'f' of foo.bar
    }

    #[test]
    fn test_word_backward() {
        let mut state = CopyModeState::new(80, 24, 100);

        let line: Vec<char> = "hello world_test  foo".chars().collect();

        // b from end
        state.cursor = BufferPos::new(20, 0);
        state.move_word_backward(&line, false);
        assert_eq!(state.cursor.x, 18); // 'f' of foo

        // b again -> "world"
        state.move_word_backward(&line, false);
        assert_eq!(state.cursor.x, 6); // 'w' of world_test

        // b again -> "hello"
        state.move_word_backward(&line, false);
        assert_eq!(state.cursor.x, 0);
    }

    #[test]
    fn test_word_end() {
        let mut state = CopyModeState::new(80, 24, 100);
        state.cursor = BufferPos::new(0, 0);

        let line: Vec<char> = "hello world foo".chars().collect();

        // e from start -> end of "hello"
        state.move_word_end(&line, false);
        assert_eq!(state.cursor.x, 4); // 'o' of hello

        // e again -> end of "world"
        state.move_word_end(&line, false);
        assert_eq!(state.cursor.x, 10); // 'd' of world
    }

    #[test]
    fn test_is_selected() {
        let mut state = CopyModeState::new(80, 24, 100);
        state.cursor = BufferPos::new(5, 10);
        state.toggle_visual_char();
        state.cursor = BufferPos::new(10, 10);
        state.selection.as_mut().unwrap().cursor = state.cursor;

        // Within selection
        assert!(state.is_selected(5, 10));
        assert!(state.is_selected(7, 10));
        assert!(state.is_selected(10, 10));

        // Outside selection
        assert!(!state.is_selected(4, 10));
        assert!(!state.is_selected(11, 10));
        assert!(!state.is_selected(5, 9));
        assert!(!state.is_selected(5, 11));
    }
}
