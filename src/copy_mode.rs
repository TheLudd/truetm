//! Vim-style copy mode for scrollback navigation and text selection

use regex::Regex;

/// Search input mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    None,
    Forward,  // /
    Backward, // ?
}

/// Find char mode for f/F/t/T
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FindChar {
    pub char: char,
    pub forward: bool,  // f/t = true, F/T = false
    pub inclusive: bool, // f/F = true (on char), t/T = false (before/after char)
}

/// Text object modifier (i = inner, a = around)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextObjectModifier {
    Inner, // i
    Around, // a
}

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
    pub count: Option<u32>,   // Numeric prefix for motions (e.g., 5j)
    // Search state
    pub search_mode: SearchMode,
    pub search_input: String,
    pub last_search: Option<(String, bool)>, // (pattern, forward)
    pub last_find: Option<FindChar>,
    pub pending_find: Option<(bool, bool)>, // (forward, inclusive) - waiting for char
    pub pending_text_object: Option<TextObjectModifier>, // i or a, waiting for object type
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
            count: None,
            search_mode: SearchMode::None,
            search_input: String::new(),
            last_search: None,
            last_find: None,
            pending_find: None,
            pending_text_object: None,
        }
    }

    /// Get the current count (default 1 if not set)
    pub fn get_count(&self) -> u32 {
        self.count.unwrap_or(1)
    }

    /// Add a digit to the count
    pub fn push_count_digit(&mut self, digit: u32) {
        let current = self.count.unwrap_or(0);
        // Limit to reasonable size to prevent overflow
        let new_count = current.saturating_mul(10).saturating_add(digit).min(99999);
        self.count = Some(new_count);
    }

    /// Reset the count after executing a motion
    pub fn reset_count(&mut self) {
        self.count = None;
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

    // === Search Methods ===

    /// Start search input mode
    pub fn start_search(&mut self, forward: bool) {
        self.search_mode = if forward { SearchMode::Forward } else { SearchMode::Backward };
        self.search_input.clear();
    }

    /// Cancel search input
    pub fn cancel_search(&mut self) {
        self.search_mode = SearchMode::None;
        self.search_input.clear();
    }

    /// Add character to search input
    pub fn search_push_char(&mut self, c: char) {
        self.search_input.push(c);
    }

    /// Remove last character from search input
    pub fn search_pop_char(&mut self) {
        self.search_input.pop();
    }

    /// Execute search and move to first match
    /// Returns true if a match was found
    pub fn execute_search<F>(&mut self, get_line: F) -> bool
    where
        F: Fn(i32) -> Vec<char>,
    {
        if self.search_input.is_empty() {
            self.search_mode = SearchMode::None;
            return false;
        }

        let forward = self.search_mode == SearchMode::Forward;
        let pattern = self.search_input.clone();
        self.last_search = Some((pattern.clone(), forward));
        self.search_mode = SearchMode::None;

        self.search_next_impl(&pattern, forward, get_line)
    }

    /// Search for next occurrence (n)
    pub fn search_next<F>(&mut self, get_line: F) -> bool
    where
        F: Fn(i32) -> Vec<char>,
    {
        if let Some((ref pattern, forward)) = self.last_search.clone() {
            self.search_next_impl(&pattern, forward, get_line)
        } else {
            false
        }
    }

    /// Search for previous occurrence (N)
    pub fn search_prev<F>(&mut self, get_line: F) -> bool
    where
        F: Fn(i32) -> Vec<char>,
    {
        if let Some((ref pattern, forward)) = self.last_search.clone() {
            self.search_next_impl(&pattern, !forward, get_line)
        } else {
            false
        }
    }

    fn search_next_impl<F>(&mut self, pattern: &str, forward: bool, get_line: F) -> bool
    where
        F: Fn(i32) -> Vec<char>,
    {
        let re = match Regex::new(pattern) {
            Ok(r) => r,
            Err(_) => return false,
        };

        let min_y = -(self.scrollback_len as i32);
        let max_y = (self.buffer_height as i32) - 1;

        // Search starting from current position
        let start_x = self.cursor.x as usize;
        let start_y = self.cursor.y;

        if forward {
            // Forward search: current line (after cursor), then subsequent lines
            let line: String = get_line(start_y).into_iter().collect();
            if let Some(m) = re.find(&line[start_x.saturating_add(1).min(line.len())..]) {
                let new_x = start_x + 1 + m.start();
                self.move_cursor(BufferPos::new(new_x as u16, start_y));
                return true;
            }

            // Search subsequent lines
            for y in (start_y + 1)..=max_y {
                let line: String = get_line(y).into_iter().collect();
                if let Some(m) = re.find(&line) {
                    self.move_cursor(BufferPos::new(m.start() as u16, y));
                    return true;
                }
            }

            // Wrap to top and search to current position
            for y in min_y..start_y {
                let line: String = get_line(y).into_iter().collect();
                if let Some(m) = re.find(&line) {
                    self.move_cursor(BufferPos::new(m.start() as u16, y));
                    return true;
                }
            }
        } else {
            // Backward search: current line (before cursor), then previous lines
            let line: String = get_line(start_y).into_iter().collect();
            let before = &line[..start_x];
            if let Some(m) = re.find_iter(before).last() {
                self.move_cursor(BufferPos::new(m.start() as u16, start_y));
                return true;
            }

            // Search previous lines
            for y in (min_y..start_y).rev() {
                let line: String = get_line(y).into_iter().collect();
                if let Some(m) = re.find_iter(&line).last() {
                    self.move_cursor(BufferPos::new(m.start() as u16, y));
                    return true;
                }
            }

            // Wrap to bottom and search to current position
            for y in ((start_y + 1)..=max_y).rev() {
                let line: String = get_line(y).into_iter().collect();
                if let Some(m) = re.find_iter(&line).last() {
                    self.move_cursor(BufferPos::new(m.start() as u16, y));
                    return true;
                }
            }
        }

        false
    }

    // === Find Char Methods (f/F/t/T) ===

    /// Start waiting for find char
    pub fn start_find_char(&mut self, forward: bool, inclusive: bool) {
        self.pending_find = Some((forward, inclusive));
    }

    /// Execute find char motion
    pub fn do_find_char(&mut self, c: char, line_content: &[char]) -> bool {
        let (forward, inclusive) = match self.pending_find.take() {
            Some(f) => f,
            None => return false,
        };

        self.last_find = Some(FindChar { char: c, forward, inclusive });
        self.find_char_impl(c, forward, inclusive, line_content)
    }

    /// Repeat last find (;)
    pub fn repeat_find(&mut self, line_content: &[char]) -> bool {
        if let Some(find) = self.last_find {
            self.find_char_impl(find.char, find.forward, find.inclusive, line_content)
        } else {
            false
        }
    }

    /// Repeat last find in opposite direction (,)
    pub fn repeat_find_reverse(&mut self, line_content: &[char]) -> bool {
        if let Some(find) = self.last_find {
            self.find_char_impl(find.char, !find.forward, find.inclusive, line_content)
        } else {
            false
        }
    }

    fn find_char_impl(&mut self, c: char, forward: bool, inclusive: bool, line_content: &[char]) -> bool {
        let x = self.cursor.x as usize;
        let len = line_content.len();

        if forward {
            // Search forward from cursor+1
            for i in (x + 1)..len {
                if line_content[i] == c {
                    let new_x = if inclusive { i } else { i.saturating_sub(1) };
                    self.move_cursor(BufferPos::new(new_x as u16, self.cursor.y));
                    return true;
                }
            }
        } else {
            // Search backward from cursor-1
            if x > 0 {
                for i in (0..x).rev() {
                    if line_content[i] == c {
                        let new_x = if inclusive { i } else { i + 1 };
                        self.move_cursor(BufferPos::new(new_x as u16, self.cursor.y));
                        return true;
                    }
                }
            }
        }

        false
    }

    // === Text Object Methods ===

    /// Start waiting for text object type
    pub fn start_text_object(&mut self, modifier: TextObjectModifier) {
        self.pending_text_object = Some(modifier);
    }

    /// Select a text object and enter visual mode if not already
    /// Returns true if selection was successful
    pub fn select_text_object(&mut self, obj_type: char, line_content: &[char]) -> bool {
        let modifier = match self.pending_text_object.take() {
            Some(m) => m,
            None => return false,
        };

        let inner = modifier == TextObjectModifier::Inner;

        match obj_type {
            'w' => self.select_word_object(line_content, inner, false),
            'W' => self.select_word_object(line_content, inner, true),
            '"' => self.select_quote_object(line_content, inner, '"'),
            '\'' => self.select_quote_object(line_content, inner, '\''),
            '`' => self.select_quote_object(line_content, inner, '`'),
            '(' | ')' | 'b' => self.select_bracket_object(line_content, inner, '(', ')'),
            '[' | ']' => self.select_bracket_object(line_content, inner, '[', ']'),
            '{' | '}' | 'B' => self.select_bracket_object(line_content, inner, '{', '}'),
            '<' | '>' => self.select_bracket_object(line_content, inner, '<', '>'),
            _ => false,
        }
    }

    fn select_word_object(&mut self, line_content: &[char], inner: bool, big_word: bool) -> bool {
        let classify = if big_word { CharClass::of_word } else { CharClass::of };
        let x = self.cursor.x as usize;
        let len = line_content.len();

        if x >= len {
            return false;
        }

        let current_class = classify(line_content[x]);

        // Find start of word
        let mut start = x;
        while start > 0 && classify(line_content[start - 1]) == current_class {
            start -= 1;
        }

        // Find end of word
        let mut end = x;
        while end + 1 < len && classify(line_content[end + 1]) == current_class {
            end += 1;
        }

        // For "around", include trailing whitespace (or leading if at end of line)
        if !inner {
            // Try trailing whitespace first
            let mut trailing_end = end;
            while trailing_end + 1 < len && line_content[trailing_end + 1].is_whitespace() {
                trailing_end += 1;
            }
            if trailing_end > end {
                end = trailing_end;
            } else {
                // No trailing whitespace, try leading
                while start > 0 && line_content[start - 1].is_whitespace() {
                    start -= 1;
                }
            }
        }

        // Enter visual mode and set selection
        self.visual_mode = VisualMode::Char;
        self.cursor = BufferPos::new(start as u16, self.cursor.y);
        self.selection = Some(Selection {
            anchor: BufferPos::new(start as u16, self.cursor.y),
            cursor: BufferPos::new(end as u16, self.cursor.y),
        });
        self.cursor = BufferPos::new(end as u16, self.cursor.y);

        true
    }

    fn select_quote_object(&mut self, line_content: &[char], inner: bool, quote: char) -> bool {
        let x = self.cursor.x as usize;
        let len = line_content.len();

        // Find opening quote (backward from cursor or at cursor)
        let mut open_pos = None;
        for i in (0..=x).rev() {
            if line_content[i] == quote {
                open_pos = Some(i);
                break;
            }
        }

        let open = match open_pos {
            Some(p) => p,
            None => return false,
        };

        // Find closing quote (forward from opening)
        let mut close_pos = None;
        for i in (open + 1)..len {
            if line_content[i] == quote {
                close_pos = Some(i);
                break;
            }
        }

        let close = match close_pos {
            Some(p) => p,
            None => return false,
        };

        let (start, end) = if inner {
            (open + 1, close.saturating_sub(1))
        } else {
            (open, close)
        };

        if start > end {
            return false;
        }

        // Enter visual mode and set selection
        self.visual_mode = VisualMode::Char;
        self.cursor = BufferPos::new(start as u16, self.cursor.y);
        self.selection = Some(Selection {
            anchor: BufferPos::new(start as u16, self.cursor.y),
            cursor: BufferPos::new(end as u16, self.cursor.y),
        });
        self.cursor = BufferPos::new(end as u16, self.cursor.y);

        true
    }

    fn select_bracket_object(&mut self, line_content: &[char], inner: bool, open: char, close: char) -> bool {
        let x = self.cursor.x as usize;
        let len = line_content.len();

        // Find opening bracket (backward, handling nesting)
        let mut open_pos = None;
        let mut depth = 0;
        for i in (0..=x).rev() {
            if line_content[i] == close && i != x {
                depth += 1;
            } else if line_content[i] == open {
                if depth == 0 {
                    open_pos = Some(i);
                    break;
                }
                depth -= 1;
            }
        }

        let open_idx = match open_pos {
            Some(p) => p,
            None => return false,
        };

        // Find closing bracket (forward, handling nesting)
        let mut close_pos = None;
        let mut depth = 0;
        for i in (open_idx + 1)..len {
            if line_content[i] == open {
                depth += 1;
            } else if line_content[i] == close {
                if depth == 0 {
                    close_pos = Some(i);
                    break;
                }
                depth -= 1;
            }
        }

        let close_idx = match close_pos {
            Some(p) => p,
            None => return false,
        };

        let (start, end) = if inner {
            (open_idx + 1, close_idx.saturating_sub(1))
        } else {
            (open_idx, close_idx)
        };

        if start > end {
            return false;
        }

        // Enter visual mode and set selection
        self.visual_mode = VisualMode::Char;
        self.cursor = BufferPos::new(start as u16, self.cursor.y);
        self.selection = Some(Selection {
            anchor: BufferPos::new(start as u16, self.cursor.y),
            cursor: BufferPos::new(end as u16, self.cursor.y),
        });
        self.cursor = BufferPos::new(end as u16, self.cursor.y);

        true
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

    #[test]
    fn test_text_object_word() {
        let mut state = CopyModeState::new(80, 24, 100);
        let line: Vec<char> = "hello world foo".chars().collect();

        // Position cursor in middle of "world"
        state.cursor = BufferPos::new(8, 0);
        state.start_text_object(TextObjectModifier::Inner);
        assert!(state.select_text_object('w', &line));

        // Should select "world" (positions 6-10)
        let bounds = state.get_selection_bounds().unwrap();
        assert_eq!(bounds.0, 6); // start x
        assert_eq!(bounds.2, 10); // end x
    }

    #[test]
    fn test_text_object_quotes() {
        let mut state = CopyModeState::new(80, 24, 100);
        let line: Vec<char> = r#"say "hello world" now"#.chars().collect();

        // Position cursor inside the quoted string
        state.cursor = BufferPos::new(8, 0);
        state.start_text_object(TextObjectModifier::Inner);
        assert!(state.select_text_object('"', &line));

        // Should select "hello world" (inside quotes)
        let bounds = state.get_selection_bounds().unwrap();
        assert_eq!(bounds.0, 5); // start x (after opening quote)
        assert_eq!(bounds.2, 15); // end x (before closing quote)
    }

    #[test]
    fn test_text_object_brackets() {
        let mut state = CopyModeState::new(80, 24, 100);
        let line: Vec<char> = "func(arg1, arg2)".chars().collect();

        // Position cursor inside parens
        state.cursor = BufferPos::new(8, 0);
        state.start_text_object(TextObjectModifier::Around);
        assert!(state.select_text_object('(', &line));

        // Should select "(arg1, arg2)" including parens
        let bounds = state.get_selection_bounds().unwrap();
        assert_eq!(bounds.0, 4); // start x (opening paren)
        assert_eq!(bounds.2, 15); // end x (closing paren)
    }
}
