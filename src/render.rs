//! Rendering - screen buffers and compositor

use crate::pane::Rect;
use crossterm::{
    cursor::MoveTo,
    queue,
    style::{Color, ResetColor, SetBackgroundColor, SetForegroundColor},
};
use std::io::Write;

/// Text attributes as bitflags
#[derive(Clone, Copy, PartialEq, Default)]
pub struct Attrs(u8);

impl Attrs {
    pub const BOLD: u8 = 1 << 0;
    pub const DIM: u8 = 1 << 1;
    pub const ITALIC: u8 = 1 << 2;
    pub const UNDERLINE: u8 = 1 << 3;
    pub const REVERSE: u8 = 1 << 4;
    pub const STRIKETHROUGH: u8 = 1 << 5;

    pub fn has(&self, attr: u8) -> bool {
        self.0 & attr != 0
    }

    pub fn set(&mut self, attr: u8) {
        self.0 |= attr;
    }

    pub fn clear(&mut self, attr: u8) {
        self.0 &= !attr;
    }

    pub fn reset(&mut self) {
        self.0 = 0;
    }
}

/// A cell in the screen buffer
#[derive(Clone, Copy, PartialEq)]
pub struct Cell {
    pub ch: char,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub attrs: Attrs,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: None,
            bg: None,
            attrs: Attrs::default(),
        }
    }
}

/// Default scrollback buffer size (number of lines)
const DEFAULT_SCROLLBACK: usize = 1000;

/// Screen buffer for a pane - stores the current display state
pub struct ScreenBuffer {
    cells: Vec<Cell>,
    width: u16,
    height: u16,
    cursor_x: u16,
    cursor_y: u16,
    // Current text attributes
    current_fg: Option<Color>,
    current_bg: Option<Color>,
    current_attrs: Attrs,
    // Parser state
    parse_state: ParseState,
    parse_buffer: Vec<u8>,
    // UTF-8 accumulator
    utf8_buffer: Vec<u8>,
    utf8_remaining: u8,
    // Alternate screen buffer support
    saved_cells: Option<Vec<Cell>>,
    saved_cursor: Option<(u16, u16)>,
    in_alternate_screen: bool,
    // Window title (set via OSC sequences)
    title: Option<String>,
    // Cursor visibility (controlled by CSI ?25h/l)
    cursor_visible: bool,
    // Scroll region (top and bottom line, 0-indexed, inclusive)
    scroll_top: u16,
    scroll_bottom: u16,
    // Scrollback buffer - lines that scrolled off the top
    scrollback: std::collections::VecDeque<Vec<Cell>>,
    scrollback_limit: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum ParseState {
    Normal,
    Escape,
    Csi,
    Osc,
    Dcs,           // Device Control String (ESC P ... ST)
    CharsetSelect, // ESC ( X, ESC ) X - consume next byte
}

impl ScreenBuffer {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            cells: vec![Cell::default(); (width as usize) * (height as usize)],
            width,
            height,
            cursor_x: 0,
            cursor_y: 0,
            current_fg: None,
            current_bg: None,
            current_attrs: Attrs::default(),
            parse_state: ParseState::Normal,
            parse_buffer: Vec::new(),
            utf8_buffer: Vec::new(),
            utf8_remaining: 0,
            saved_cells: None,
            saved_cursor: None,
            in_alternate_screen: false,
            title: None,
            cursor_visible: true,
            scroll_top: 0,
            scroll_bottom: height.saturating_sub(1),
            scrollback: std::collections::VecDeque::new(),
            scrollback_limit: DEFAULT_SCROLLBACK,
        }
    }

    /// Resize the buffer
    pub fn resize(&mut self, width: u16, height: u16) {
        if width == self.width && height == self.height {
            return;
        }

        let mut new_cells = vec![Cell::default(); (width as usize) * (height as usize)];

        // Copy existing content
        for y in 0..self.height.min(height) {
            for x in 0..self.width.min(width) {
                let old_idx = (y as usize) * (self.width as usize) + (x as usize);
                let new_idx = (y as usize) * (width as usize) + (x as usize);
                new_cells[new_idx] = self.cells[old_idx];
            }
        }

        self.cells = new_cells;
        self.width = width;
        self.height = height;
        self.cursor_x = self.cursor_x.min(width.saturating_sub(1));
        self.cursor_y = self.cursor_y.min(height.saturating_sub(1));
        // Reset scroll region to full screen
        self.scroll_top = 0;
        self.scroll_bottom = height.saturating_sub(1);
    }

    /// Process raw bytes from PTY
    pub fn process(&mut self, data: &[u8]) {
        for &byte in data {
            match self.parse_state {
                ParseState::Normal => self.process_normal(byte),
                ParseState::Escape => self.process_escape(byte),
                ParseState::Csi => self.process_csi(byte),
                ParseState::Osc => self.process_osc(byte),
                ParseState::Dcs => self.process_dcs(byte),
                ParseState::CharsetSelect => self.process_charset_select(byte),
            }
        }
    }

    fn process_normal(&mut self, byte: u8) {
        // If we're in the middle of a UTF-8 sequence
        if self.utf8_remaining > 0 {
            if byte & 0xC0 == 0x80 {
                // Valid continuation byte
                self.utf8_buffer.push(byte);
                self.utf8_remaining -= 1;
                if self.utf8_remaining == 0 {
                    // Complete UTF-8 sequence - copy buffer to avoid borrow issues
                    let buf = std::mem::take(&mut self.utf8_buffer);
                    if let Ok(s) = std::str::from_utf8(&buf) {
                        for ch in s.chars() {
                            self.put_char(ch);
                        }
                    }
                }
            } else {
                // Invalid continuation - discard and process this byte normally
                self.utf8_buffer.clear();
                self.utf8_remaining = 0;
                self.process_normal(byte);
            }
            return;
        }

        match byte {
            0x1b => {
                self.parse_state = ParseState::Escape;
                self.parse_buffer.clear();
            }
            0x07 => {} // Bell - ignore
            0x08 => {
                // Backspace
                self.cursor_x = self.cursor_x.saturating_sub(1);
            }
            0x09 => {
                // Tab
                self.cursor_x = ((self.cursor_x / 8) + 1) * 8;
                if self.cursor_x >= self.width {
                    self.cursor_x = self.width.saturating_sub(1);
                }
            }
            0x0a => {
                // Line feed
                self.line_feed();
            }
            0x0d => {
                // Carriage return
                self.cursor_x = 0;
            }
            // UTF-8 multi-byte sequence starters
            0xC0..=0xDF => {
                // 2-byte sequence
                self.utf8_buffer.clear();
                self.utf8_buffer.push(byte);
                self.utf8_remaining = 1;
            }
            0xE0..=0xEF => {
                // 3-byte sequence
                self.utf8_buffer.clear();
                self.utf8_buffer.push(byte);
                self.utf8_remaining = 2;
            }
            0xF0..=0xF7 => {
                // 4-byte sequence
                self.utf8_buffer.clear();
                self.utf8_buffer.push(byte);
                self.utf8_remaining = 3;
            }
            0x20..=0x7E => {
                // Printable ASCII
                self.put_char(byte as char);
            }
            _ => {} // Ignore other control chars and invalid bytes
        }
    }

    fn process_escape(&mut self, byte: u8) {
        match byte {
            b'[' => {
                self.parse_state = ParseState::Csi;
                self.parse_buffer.clear();
            }
            b']' => {
                self.parse_state = ParseState::Osc;
                self.parse_buffer.clear();
            }
            b'P' => {
                // DCS - Device Control String
                self.parse_state = ParseState::Dcs;
                self.parse_buffer.clear();
            }
            b'_' | b'^' | b'X' => {
                // APC, PM, SOS - consume until ST, treat like DCS
                self.parse_state = ParseState::Dcs;
                self.parse_buffer.clear();
            }
            b'(' | b')' | b'*' | b'+' | b'-' | b'.' | b'/' => {
                // Character set designation - next byte is the charset ID
                // ESC ( B = G0 to ASCII, ESC ) 0 = G1 to DEC graphics, etc.
                // ESC - / . / / are for 96-character sets (VT220+)
                // We need to consume the next byte too
                self.parse_state = ParseState::CharsetSelect;
            }
            b'M' => {
                // Reverse line feed
                if self.cursor_y > 0 {
                    self.cursor_y -= 1;
                }
                self.parse_state = ParseState::Normal;
            }
            b'7' | b's' => {
                // Save cursor (DECSC or ANSI)
                self.saved_cursor = Some((self.cursor_x, self.cursor_y));
                self.parse_state = ParseState::Normal;
            }
            b'8' | b'u' => {
                // Restore cursor (DECRC or ANSI)
                if let Some((x, y)) = self.saved_cursor {
                    self.cursor_x = x.min(self.width.saturating_sub(1));
                    self.cursor_y = y.min(self.height.saturating_sub(1));
                }
                self.parse_state = ParseState::Normal;
            }
            b'\\' => {
                // ST (String Terminator) - just return to normal
                self.parse_state = ParseState::Normal;
            }
            b'c' => {
                // RIS - Full Reset
                self.erase_all();
                self.cursor_x = 0;
                self.cursor_y = 0;
                self.current_fg = None;
                self.current_bg = None;
                self.current_attrs.reset();
                self.parse_state = ParseState::Normal;
            }
            b'D' => {
                // IND - Index (move down, scroll if needed)
                self.line_feed();
                self.parse_state = ParseState::Normal;
            }
            b'E' => {
                // NEL - Next Line
                self.cursor_x = 0;
                self.line_feed();
                self.parse_state = ParseState::Normal;
            }
            b'H' => {
                // HTS - Horizontal Tab Set (ignore)
                self.parse_state = ParseState::Normal;
            }
            b'=' | b'>' => {
                // DECKPAM/DECKPNM - Keypad modes (ignore)
                self.parse_state = ParseState::Normal;
            }
            _ => {
                self.parse_state = ParseState::Normal;
            }
        }
    }

    fn process_charset_select(&mut self, _byte: u8) {
        // Just consume the charset identifier byte (B, 0, A, etc.)
        // We don't actually change character sets, just ignore
        self.parse_state = ParseState::Normal;
    }

    fn process_dcs(&mut self, byte: u8) {
        // DCS sequences end with ST (ESC \) or BEL
        if byte == 0x07 {
            // BEL terminates
            self.parse_state = ParseState::Normal;
        } else if byte == 0x1b {
            // Might be start of ST (ESC \)
            self.parse_buffer.push(byte);
        } else if !self.parse_buffer.is_empty() && *self.parse_buffer.last().unwrap() == 0x1b && byte == b'\\' {
            // ST received, end DCS
            self.parse_state = ParseState::Normal;
        } else if byte == 0x9c {
            // C1 ST
            self.parse_state = ParseState::Normal;
        } else {
            // Consume but don't store (we ignore DCS content)
            // Clear buffer if it wasn't an ESC
            if !self.parse_buffer.is_empty() && *self.parse_buffer.last().unwrap() == 0x1b {
                self.parse_buffer.clear();
            }
            // Safety limit
            if self.parse_buffer.len() > 4096 {
                self.parse_state = ParseState::Normal;
            }
        }
    }

    fn process_csi(&mut self, byte: u8) {
        if byte >= 0x40 && byte <= 0x7e {
            // Final byte - execute sequence
            self.parse_buffer.push(byte);
            self.execute_csi();
            self.parse_state = ParseState::Normal;
        } else {
            self.parse_buffer.push(byte);
            // Sanity limit
            if self.parse_buffer.len() > 64 {
                self.parse_state = ParseState::Normal;
            }
        }
    }

    fn process_osc(&mut self, byte: u8) {
        // OSC sequences end with BEL (0x07) or ST (ESC \)
        if byte == 0x07 {
            self.execute_osc();
            self.parse_state = ParseState::Normal;
        } else if byte == 0x1b {
            // Might be ST
            self.parse_buffer.push(byte);
        } else if !self.parse_buffer.is_empty() && *self.parse_buffer.last().unwrap() == 0x1b && byte == b'\\' {
            // Remove the ESC we added
            self.parse_buffer.pop();
            self.execute_osc();
            self.parse_state = ParseState::Normal;
        } else {
            self.parse_buffer.push(byte);
            if self.parse_buffer.len() > 256 {
                self.parse_state = ParseState::Normal;
            }
        }
    }

    fn execute_osc(&mut self) {
        // OSC format: Ps ; Pt where Ps is command number, Pt is parameter text
        if let Ok(s) = std::str::from_utf8(&self.parse_buffer) {
            if let Some((cmd, text)) = s.split_once(';') {
                match cmd {
                    "0" | "1" | "2" => {
                        // 0 = set icon name and window title
                        // 1 = set icon name
                        // 2 = set window title
                        self.title = Some(text.to_string());
                    }
                    _ => {} // Ignore other OSC commands
                }
            }
        }
        self.parse_buffer.clear();
    }

    fn execute_csi(&mut self) {
        if self.parse_buffer.is_empty() {
            return;
        }

        let final_byte = *self.parse_buffer.last().unwrap();
        let params_str = String::from_utf8_lossy(&self.parse_buffer[..self.parse_buffer.len() - 1]);

        // For SGR (m), we need special handling of colon sub-parameters
        // For other sequences, just parse semicolon-separated values
        if final_byte == b'm' {
            let params_owned = params_str.to_string();
            self.process_sgr_string(&params_owned);
            return;
        }

        let params: Vec<u16> = params_str
            .split(';')
            .filter_map(|s| s.parse().ok())
            .collect();

        match final_byte {
            b'A' => {
                // Cursor up
                let n = params.first().copied().unwrap_or(1).max(1);
                self.cursor_y = self.cursor_y.saturating_sub(n);
            }
            b'B' => {
                // Cursor down
                let n = params.first().copied().unwrap_or(1).max(1);
                self.cursor_y = (self.cursor_y + n).min(self.height.saturating_sub(1));
            }
            b'C' => {
                // Cursor forward
                let n = params.first().copied().unwrap_or(1).max(1);
                self.cursor_x = (self.cursor_x + n).min(self.width.saturating_sub(1));
            }
            b'D' => {
                // Cursor back
                let n = params.first().copied().unwrap_or(1).max(1);
                self.cursor_x = self.cursor_x.saturating_sub(n);
            }
            b'H' | b'f' => {
                // Cursor position
                let row = params.first().copied().unwrap_or(1).max(1) - 1;
                let col = params.get(1).copied().unwrap_or(1).max(1) - 1;
                self.cursor_y = row.min(self.height.saturating_sub(1));
                self.cursor_x = col.min(self.width.saturating_sub(1));
            }
            b'J' => {
                // Erase in display
                let mode = params.first().copied().unwrap_or(0);
                match mode {
                    0 => self.erase_below(),
                    1 => self.erase_above(),
                    2 | 3 => self.erase_all(),
                    _ => {}
                }
            }
            b'K' => {
                // Erase in line
                let mode = params.first().copied().unwrap_or(0);
                match mode {
                    0 => self.erase_line_right(),
                    1 => self.erase_line_left(),
                    2 => self.erase_line(),
                    _ => {}
                }
            }
            b'L' => {
                // Insert lines
                let n = params.first().copied().unwrap_or(1).max(1) as usize;
                self.insert_lines(n);
            }
            b'M' => {
                // Delete lines
                let n = params.first().copied().unwrap_or(1).max(1) as usize;
                self.delete_lines(n);
            }
            b'r' => {
                // DECSTBM - Set Top and Bottom Margins (scroll region)
                let top = params.first().copied().unwrap_or(1).max(1) - 1;
                let bottom = params.get(1).copied().unwrap_or(self.height).max(1) - 1;
                if top < bottom && bottom < self.height {
                    self.scroll_top = top;
                    self.scroll_bottom = bottom;
                    // Cursor moves to home position after setting scroll region
                    self.cursor_x = 0;
                    self.cursor_y = 0;
                }
            }
            b'G' => {
                // Cursor horizontal absolute (column)
                let col = params.first().copied().unwrap_or(1).max(1) - 1;
                self.cursor_x = col.min(self.width.saturating_sub(1));
            }
            b's' => {
                // Save cursor position (ANSI)
                self.saved_cursor = Some((self.cursor_x, self.cursor_y));
            }
            b'u' => {
                // Restore cursor position (ANSI)
                if let Some((x, y)) = self.saved_cursor {
                    self.cursor_x = x.min(self.width.saturating_sub(1));
                    self.cursor_y = y.min(self.height.saturating_sub(1));
                }
            }
            b'd' => {
                // Cursor vertical absolute (line)
                let row = params.first().copied().unwrap_or(1).max(1) - 1;
                self.cursor_y = row.min(self.height.saturating_sub(1));
            }
            b'h' | b'l' => {
                // Set/reset mode
                let is_set = final_byte == b'h';
                // Check for private mode (? prefix)
                if params_str.starts_with('?') {
                    let mode: Option<u16> = params_str[1..].parse().ok();
                    if let Some(mode) = mode {
                        self.handle_private_mode(mode, is_set);
                    }
                }
            }
            b'X' => {
                // Erase characters (replace with spaces)
                let n = params.first().copied().unwrap_or(1).max(1) as usize;
                let blank = self.blank_cell();
                let y = self.cursor_y as usize;
                let width = self.width as usize;
                for i in 0..n {
                    let x = self.cursor_x as usize + i;
                    if x < width {
                        self.cells[y * width + x] = blank;
                    }
                }
            }
            b'P' => {
                // Delete characters (shift left)
                let n = params.first().copied().unwrap_or(1).max(1) as usize;
                let y = self.cursor_y as usize;
                let width = self.width as usize;
                let x = self.cursor_x as usize;
                let blank = self.blank_cell();
                // Shift cells left
                for i in x..width.saturating_sub(n) {
                    self.cells[y * width + i] = self.cells[y * width + i + n];
                }
                // Fill end with blanks
                for i in width.saturating_sub(n)..width {
                    self.cells[y * width + i] = blank;
                }
            }
            b'@' => {
                // Insert characters (shift right)
                let n = params.first().copied().unwrap_or(1).max(1) as usize;
                let y = self.cursor_y as usize;
                let width = self.width as usize;
                let x = self.cursor_x as usize;
                let blank = self.blank_cell();
                // Shift cells right
                for i in (x + n..width).rev() {
                    self.cells[y * width + i] = self.cells[y * width + i - n];
                }
                // Fill with blanks
                for i in x..(x + n).min(width) {
                    self.cells[y * width + i] = blank;
                }
            }
            b'q' => {
                // DECSCUSR - Set cursor style (ignore, we manage cursor ourselves)
                // CSI Ps SP q - but SP is space (0x20), handled separately
            }
            b' ' => {
                // Could be start of CSI Ps SP q sequence - already consumed
                // The 'q' would be the final byte, but we got ' ' as final
                // This means malformed sequence, ignore
            }
            b'S' => {
                // SU - Scroll Up (pan down)
                let n = params.first().copied().unwrap_or(1).max(1);
                for _ in 0..n {
                    self.scroll_up();
                }
            }
            b'T' => {
                // SD - Scroll Down (pan up)
                let n = params.first().copied().unwrap_or(1).max(1);
                for _ in 0..n {
                    self.scroll_down();
                }
            }
            _ => {}
        }
    }

    fn handle_private_mode(&mut self, mode: u16, is_set: bool) {
        match mode {
            25 => {
                // Cursor visibility
                self.cursor_visible = is_set;
            }
            1049 => {
                // Alternate screen buffer with saved cursor
                if is_set {
                    self.enter_alternate_screen();
                } else {
                    self.leave_alternate_screen();
                }
            }
            47 | 1047 => {
                // Alternate screen buffer (without cursor save)
                if is_set {
                    self.enter_alternate_screen();
                } else {
                    self.leave_alternate_screen();
                }
            }
            _ => {} // Ignore other private modes
        }
    }

    fn enter_alternate_screen(&mut self) {
        if self.in_alternate_screen {
            return;
        }
        // Save current screen and cursor
        self.saved_cells = Some(self.cells.clone());
        self.saved_cursor = Some((self.cursor_x, self.cursor_y));
        self.in_alternate_screen = true;
        // Clear the screen for the alternate buffer
        self.erase_all();
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    fn leave_alternate_screen(&mut self) {
        if !self.in_alternate_screen {
            return;
        }
        // Restore saved screen and cursor
        if let Some(saved) = self.saved_cells.take() {
            // Only restore if dimensions match
            if saved.len() == self.cells.len() {
                self.cells = saved;
            } else {
                // Dimensions changed, just clear
                self.erase_all();
            }
        } else {
            self.erase_all();
        }
        if let Some((x, y)) = self.saved_cursor.take() {
            self.cursor_x = x.min(self.width.saturating_sub(1));
            self.cursor_y = y.min(self.height.saturating_sub(1));
        } else {
            self.cursor_x = 0;
            self.cursor_y = 0;
        }
        self.in_alternate_screen = false;
    }

    /// Process SGR string with proper colon sub-parameter handling
    fn process_sgr_string(&mut self, params_str: &str) {
        if params_str.is_empty() {
            // ESC[m = reset
            self.current_fg = None;
            self.current_bg = None;
            self.current_attrs.reset();
            return;
        }

        // Handle colon sub-parameters by converting to semicolons
        // But special-case underline styles (4:N) to just set underline
        let mut processed = String::with_capacity(params_str.len());
        for segment in params_str.split(';') {
            if !processed.is_empty() {
                processed.push(';');
            }

            if segment.contains(':') {
                // Has colon sub-parameters
                let parts: Vec<&str> = segment.split(':').collect();
                if let Some(first) = parts.first() {
                    match *first {
                        "4" => {
                            // Underline style - check if disabling (4:0) or enabling
                            if parts.get(1) == Some(&"0") {
                                processed.push_str("24"); // Turn off underline
                            } else {
                                processed.push_str("4"); // Just enable underline
                            }
                        }
                        "58" => {
                            // Underline color - ignore entirely
                        }
                        "38" | "48" => {
                            // Color with colon separator - convert to semicolon format
                            processed.push_str(&parts.join(";"));
                        }
                        _ => {
                            // Unknown - just use first part
                            processed.push_str(first);
                        }
                    }
                }
            } else {
                processed.push_str(segment);
            }
        }

        // Now parse as normal semicolon-separated params
        let params: Vec<u16> = processed
            .split(';')
            .filter_map(|s| s.parse().ok())
            .collect();

        self.process_sgr(&params);
    }

    fn process_sgr(&mut self, params: &[u16]) {
        if params.is_empty() {
            // Reset all
            self.current_fg = None;
            self.current_bg = None;
            self.current_attrs.reset();
            return;
        }

        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => {
                    // Reset all
                    self.current_fg = None;
                    self.current_bg = None;
                    self.current_attrs.reset();
                }
                1 => self.current_attrs.set(Attrs::BOLD),
                2 => self.current_attrs.set(Attrs::DIM),
                3 => self.current_attrs.set(Attrs::ITALIC),
                4 => self.current_attrs.set(Attrs::UNDERLINE),
                5 | 6 => {} // Blink - ignore
                7 => self.current_attrs.set(Attrs::REVERSE),
                8 => {} // Hidden/invisible - ignore
                9 => self.current_attrs.set(Attrs::STRIKETHROUGH),
                21 => self.current_attrs.clear(Attrs::BOLD), // Double underline or bold off
                22 => {
                    self.current_attrs.clear(Attrs::BOLD);
                    self.current_attrs.clear(Attrs::DIM);
                }
                23 => self.current_attrs.clear(Attrs::ITALIC),
                24 => self.current_attrs.clear(Attrs::UNDERLINE),
                27 => self.current_attrs.clear(Attrs::REVERSE),
                29 => self.current_attrs.clear(Attrs::STRIKETHROUGH),
                30..=37 => self.current_fg = Some(ansi_to_color(params[i] - 30)),
                38 => {
                    // Extended foreground
                    if i + 2 < params.len() && params[i + 1] == 5 {
                        // 256 color
                        self.current_fg = Some(Color::AnsiValue(params[i + 2] as u8));
                        i += 2;
                    } else if i + 4 < params.len() && params[i + 1] == 2 {
                        // Truecolor
                        self.current_fg = Some(Color::Rgb {
                            r: params[i + 2] as u8,
                            g: params[i + 3] as u8,
                            b: params[i + 4] as u8,
                        });
                        i += 4;
                    }
                }
                39 => self.current_fg = None,
                40..=47 => self.current_bg = Some(ansi_to_color(params[i] - 40)),
                48 => {
                    // Extended background
                    if i + 2 < params.len() && params[i + 1] == 5 {
                        // 256 color
                        self.current_bg = Some(Color::AnsiValue(params[i + 2] as u8));
                        i += 2;
                    } else if i + 4 < params.len() && params[i + 1] == 2 {
                        // Truecolor
                        self.current_bg = Some(Color::Rgb {
                            r: params[i + 2] as u8,
                            g: params[i + 3] as u8,
                            b: params[i + 4] as u8,
                        });
                        i += 4;
                    }
                }
                49 => self.current_bg = None,
                90..=97 => self.current_fg = Some(ansi_to_bright_color(params[i] - 90)),
                100..=107 => self.current_bg = Some(ansi_to_bright_color(params[i] - 100)),
                _ => {}
            }
            i += 1;
        }
    }

    fn put_char(&mut self, ch: char) {
        if self.cursor_x >= self.width {
            self.cursor_x = 0;
            self.line_feed();
        }

        let idx = (self.cursor_y as usize) * (self.width as usize) + (self.cursor_x as usize);
        if idx < self.cells.len() {
            self.cells[idx] = Cell {
                ch,
                fg: self.current_fg,
                bg: self.current_bg,
                attrs: self.current_attrs,
            };
        }
        self.cursor_x += 1;
    }

    fn line_feed(&mut self) {
        if self.cursor_y >= self.scroll_bottom {
            // At bottom of scroll region - scroll up
            self.scroll_up();
        } else if self.cursor_y + 1 < self.height {
            self.cursor_y += 1;
        }
    }

    fn scroll_up(&mut self) {
        // Scroll within the scroll region only
        let width = self.width as usize;
        let top = self.scroll_top as usize;
        let bottom = self.scroll_bottom as usize;

        // Save top line to scrollback if scrolling from top and not in alternate screen
        if top == 0 && !self.in_alternate_screen {
            // Use slice copy for efficiency
            let line = self.cells[0..width].to_vec();
            self.scrollback.push_back(line);
            // Trim scrollback if over limit
            if self.scrollback.len() > self.scrollback_limit {
                self.scrollback.pop_front();
            }
        }

        // Move lines up within scroll region using copy_within for efficiency
        let src_start = (top + 1) * width;
        let src_end = (bottom + 1) * width;
        let dst_start = top * width;
        self.cells.copy_within(src_start..src_end, dst_start);

        // Clear bottom line of scroll region
        let blank = self.blank_cell();
        let last_row = bottom * width;
        for cell in &mut self.cells[last_row..last_row + width] {
            *cell = blank;
        }
    }

    fn scroll_down(&mut self) {
        // Scroll down within the scroll region only
        let width = self.width as usize;
        let top = self.scroll_top as usize;
        let bottom = self.scroll_bottom as usize;

        // Move lines down within scroll region (must go in reverse to avoid overwriting)
        // copy_within doesn't work here because regions overlap in the wrong direction
        for y in (top + 1..=bottom).rev() {
            let src = (y - 1) * width;
            let dst = y * width;
            self.cells.copy_within(src..src + width, dst);
        }
        // Clear top line of scroll region
        let blank = self.blank_cell();
        let first_row = top * width;
        for cell in &mut self.cells[first_row..first_row + width] {
            *cell = blank;
        }
    }

    fn insert_lines(&mut self, n: usize) {
        let y = self.cursor_y as usize;
        let width = self.width as usize;
        let bottom = self.scroll_bottom as usize;

        // Only operate within scroll region and if cursor is in region
        if y < self.scroll_top as usize || y > bottom {
            return;
        }

        // Move lines down within scroll region (reverse order to avoid overwriting)
        for row in (y..=bottom.saturating_sub(n)).rev() {
            let src = row * width;
            let dst = (row + n) * width;
            self.cells.copy_within(src..src + width, dst);
        }
        // Clear inserted lines
        let blank = self.blank_cell();
        for row in y..(y + n).min(bottom + 1) {
            let start = row * width;
            for cell in &mut self.cells[start..start + width] {
                *cell = blank;
            }
        }
    }

    fn delete_lines(&mut self, n: usize) {
        let y = self.cursor_y as usize;
        let width = self.width as usize;
        let bottom = self.scroll_bottom as usize;

        // Only operate within scroll region and if cursor is in region
        if y < self.scroll_top as usize || y > bottom {
            return;
        }

        // Move lines up within scroll region using copy_within
        let src_start = (y + n) * width;
        let src_end = (bottom + 1) * width;
        let dst_start = y * width;
        if src_start < src_end {
            self.cells.copy_within(src_start..src_end, dst_start);
        }

        // Clear bottom lines of scroll region
        let blank = self.blank_cell();
        for row in (bottom + 1).saturating_sub(n)..=bottom {
            let start = row * width;
            for cell in &mut self.cells[start..start + width] {
                *cell = blank;
            }
        }
    }

    /// Create a blank cell with current background color (for erase operations)
    fn blank_cell(&self) -> Cell {
        Cell {
            ch: ' ',
            fg: None,
            bg: self.current_bg,
            attrs: Attrs::default(),
        }
    }

    fn erase_all(&mut self) {
        let blank = self.blank_cell();
        for cell in &mut self.cells {
            *cell = blank;
        }
    }

    fn erase_below(&mut self) {
        // Erase from cursor to end
        self.erase_line_right();
        let blank = self.blank_cell();
        let width = self.width as usize;
        let start = (self.cursor_y as usize + 1) * width;
        for cell in &mut self.cells[start..] {
            *cell = blank;
        }
    }

    fn erase_above(&mut self) {
        // Erase from start to cursor
        self.erase_line_left();
        let blank = self.blank_cell();
        let width = self.width as usize;
        let end = self.cursor_y as usize * width;
        for cell in &mut self.cells[..end] {
            *cell = blank;
        }
    }

    fn erase_line(&mut self) {
        let blank = self.blank_cell();
        let y = self.cursor_y as usize;
        let width = self.width as usize;
        let start = y * width;
        for cell in &mut self.cells[start..start + width] {
            *cell = blank;
        }
    }

    fn erase_line_right(&mut self) {
        let blank = self.blank_cell();
        let y = self.cursor_y as usize;
        let width = self.width as usize;
        let start = y * width + self.cursor_x as usize;
        let end = (y + 1) * width;
        for cell in &mut self.cells[start..end] {
            *cell = blank;
        }
    }

    fn erase_line_left(&mut self) {
        let blank = self.blank_cell();
        let y = self.cursor_y as usize;
        let width = self.width as usize;
        let start = y * width;
        let end = start + (self.cursor_x as usize + 1).min(width);
        for cell in &mut self.cells[start..end] {
            *cell = blank;
        }
    }

    /// Get cursor position
    pub fn cursor(&self) -> (u16, u16) {
        (self.cursor_x, self.cursor_y)
    }

    /// Get a cell
    pub fn get(&self, x: u16, y: u16) -> Cell {
        let idx = (y as usize) * (self.width as usize) + (x as usize);
        self.cells.get(idx).copied().unwrap_or_default()
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    /// Get number of lines in scrollback buffer
    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    /// Get a cell with scroll offset (for viewing scrollback)
    /// scroll_offset is how many lines back from current view (0 = live view)
    pub fn get_scrolled(&self, x: u16, y: u16, scroll_offset: usize) -> Cell {
        if scroll_offset == 0 {
            // Live view - use current cells
            return self.get(x, y);
        }

        let height = self.height as usize;
        let scrollback_len = self.scrollback.len();

        // Calculate which line we're looking at
        // scroll_offset lines back means: the visible screen is scrolled up
        // So the top of the screen shows scrollback[scrollback_len - scroll_offset]
        let y = y as usize;
        let x = x as usize;

        if y < scroll_offset && scroll_offset <= scrollback_len {
            // This row is in the scrollback buffer
            let scrollback_idx = scrollback_len - scroll_offset + y;
            if let Some(line) = self.scrollback.get(scrollback_idx) {
                if x < line.len() {
                    return line[x];
                }
            }
            Cell::default()
        } else if y >= scroll_offset {
            // This row is in the current screen buffer
            let screen_y = y - scroll_offset;
            if screen_y < height {
                return self.get(x as u16, screen_y as u16);
            }
            Cell::default()
        } else {
            // Scrolled past the beginning of scrollback
            Cell::default()
        }
    }
}

fn ansi_to_color(n: u16) -> Color {
    match n {
        0 => Color::Black,
        1 => Color::DarkRed,
        2 => Color::DarkGreen,
        3 => Color::DarkYellow,
        4 => Color::DarkBlue,
        5 => Color::DarkMagenta,
        6 => Color::DarkCyan,
        7 => Color::Grey,
        _ => Color::White,
    }
}

fn ansi_to_bright_color(n: u16) -> Color {
    match n {
        0 => Color::DarkGrey,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::White,
        _ => Color::White,
    }
}

/// Cell with focus state for differential rendering
#[derive(Clone, Copy, PartialEq)]
struct RenderedCell {
    cell: Cell,
    focused: bool,
}

impl Default for RenderedCell {
    fn default() -> Self {
        Self {
            cell: Cell::default(),
            focused: true,
        }
    }
}

/// Compositor renders all pane buffers to the terminal
pub struct Compositor {
    width: u16,
    height: u16,
    // Track what's currently on screen to minimize updates
    last_frame: Vec<RenderedCell>,
}

impl Compositor {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            last_frame: vec![RenderedCell::default(); (width as usize) * (height as usize)],
        }
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        // Fill with a marker that forces redraw
        self.last_frame = vec![RenderedCell::default(); (width as usize) * (height as usize)];
    }

    /// Mark the entire frame as dirty (forces full redraw)
    pub fn invalidate(&mut self) {
        for cell in &mut self.last_frame {
            cell.cell.ch = '\x00'; // Invalid char forces redraw
        }
    }

    /// Render a pane's buffer to the given rect with differential updates
    pub fn render_pane<W: Write>(
        &mut self,
        writer: &mut W,
        buffer: &ScreenBuffer,
        rect: Rect,
        focused: bool,
        scroll_offset: usize,
    ) -> std::io::Result<()> {
        use crossterm::style::{Attribute, SetAttribute};

        let mut last_fg: Option<Color> = None;
        let mut last_bg: Option<Color> = None;
        let mut last_attrs = Attrs::default();
        let mut need_move = true;
        let mut attrs_applied = false;

        for y in 0..rect.height.min(buffer.height()) {
            let screen_y = rect.y + y;

            for x in 0..rect.width.min(buffer.width()) {
                let screen_x = rect.x + x;
                let cell = buffer.get_scrolled(x, y, scroll_offset);

                // Check if this cell needs updating
                let screen_idx = (screen_y as usize) * (self.width as usize) + (screen_x as usize);
                if screen_idx < self.last_frame.len() {
                    let last = &self.last_frame[screen_idx];
                    if last.cell == cell && last.focused == focused {
                        // Cell unchanged, skip it
                        need_move = true;
                        continue;
                    }
                }

                // Cell changed - emit it
                if need_move {
                    queue!(writer, MoveTo(screen_x, screen_y))?;
                    need_move = false;
                    // Reset state after move
                    last_fg = None;
                    last_bg = None;
                    last_attrs = Attrs::default();
                    attrs_applied = false;
                }

                // Check if we need to update attributes or colors
                let attrs_changed = cell.attrs != last_attrs;
                let colors_changed = cell.fg != last_fg || cell.bg != last_bg;

                if attrs_changed || colors_changed || !attrs_applied {
                    // Reset all attributes first
                    queue!(writer, SetAttribute(Attribute::Reset))?;
                    queue!(writer, ResetColor)?;

                    // Apply cell attributes
                    if cell.attrs.has(Attrs::BOLD) {
                        queue!(writer, SetAttribute(Attribute::Bold))?;
                    }
                    if cell.attrs.has(Attrs::DIM) || !focused {
                        queue!(writer, SetAttribute(Attribute::Dim))?;
                    }
                    if cell.attrs.has(Attrs::ITALIC) {
                        queue!(writer, SetAttribute(Attribute::Italic))?;
                    }
                    if cell.attrs.has(Attrs::UNDERLINE) {
                        queue!(writer, SetAttribute(Attribute::Underlined))?;
                    }
                    if cell.attrs.has(Attrs::REVERSE) {
                        queue!(writer, SetAttribute(Attribute::Reverse))?;
                    }
                    if cell.attrs.has(Attrs::STRIKETHROUGH) {
                        queue!(writer, SetAttribute(Attribute::CrossedOut))?;
                    }

                    // Apply colors
                    if let Some(fg) = cell.fg {
                        queue!(writer, SetForegroundColor(fg))?;
                    }
                    if let Some(bg) = cell.bg {
                        queue!(writer, SetBackgroundColor(bg))?;
                    }

                    last_fg = cell.fg;
                    last_bg = cell.bg;
                    last_attrs = cell.attrs;
                    attrs_applied = true;
                }

                write!(writer, "{}", cell.ch)?;

                // Update last_frame
                if screen_idx < self.last_frame.len() {
                    self.last_frame[screen_idx] = RenderedCell { cell, focused };
                }
            }

            // End of row - next row needs MoveTo
            need_move = true;
        }

        // Reset attributes at end
        queue!(writer, SetAttribute(Attribute::Reset))?;
        queue!(writer, ResetColor)?;

        Ok(())
    }
}
