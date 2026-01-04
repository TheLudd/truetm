//! Rendering - screen buffers and compositor

use crate::pane::Rect;
use crossterm::{
    cursor::MoveTo,
    queue,
    style::{Color, ResetColor, SetBackgroundColor, SetForegroundColor},
};
use std::io::Write;

/// A cell in the screen buffer
#[derive(Clone, Copy)]
pub struct Cell {
    pub ch: char,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: None,
            bg: None,
        }
    }
}

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
    // Parser state
    parse_state: ParseState,
    parse_buffer: Vec<u8>,
    // UTF-8 accumulator
    utf8_buffer: Vec<u8>,
    utf8_remaining: u8,
}

#[derive(Clone, Copy, PartialEq)]
enum ParseState {
    Normal,
    Escape,
    Csi,
    Osc,
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
            parse_state: ParseState::Normal,
            parse_buffer: Vec::new(),
            utf8_buffer: Vec::new(),
            utf8_remaining: 0,
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
    }

    /// Process raw bytes from PTY
    pub fn process(&mut self, data: &[u8]) {
        for &byte in data {
            match self.parse_state {
                ParseState::Normal => self.process_normal(byte),
                ParseState::Escape => self.process_escape(byte),
                ParseState::Csi => self.process_csi(byte),
                ParseState::Osc => self.process_osc(byte),
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
            b'M' => {
                // Reverse line feed
                if self.cursor_y > 0 {
                    self.cursor_y -= 1;
                }
                self.parse_state = ParseState::Normal;
            }
            b'7' => {
                // Save cursor - ignore for now
                self.parse_state = ParseState::Normal;
            }
            b'8' => {
                // Restore cursor - ignore for now
                self.parse_state = ParseState::Normal;
            }
            _ => {
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
            // Ignore OSC content for now (title setting, etc.)
            self.parse_state = ParseState::Normal;
        } else if byte == 0x1b {
            // Might be ST
            self.parse_buffer.push(byte);
        } else if !self.parse_buffer.is_empty() && *self.parse_buffer.last().unwrap() == 0x1b && byte == b'\\' {
            self.parse_state = ParseState::Normal;
        } else {
            self.parse_buffer.push(byte);
            if self.parse_buffer.len() > 256 {
                self.parse_state = ParseState::Normal;
            }
        }
    }

    fn execute_csi(&mut self) {
        if self.parse_buffer.is_empty() {
            return;
        }

        let final_byte = *self.parse_buffer.last().unwrap();
        let params_str = String::from_utf8_lossy(&self.parse_buffer[..self.parse_buffer.len() - 1]);
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
            b'm' => {
                // SGR - Select Graphic Rendition
                self.process_sgr(&params);
            }
            b'r' => {
                // Set scrolling region - ignore for now
            }
            b'h' | b'l' => {
                // Set/reset mode - ignore for now
            }
            _ => {}
        }
    }

    fn process_sgr(&mut self, params: &[u16]) {
        if params.is_empty() {
            // Reset
            self.current_fg = None;
            self.current_bg = None;
            return;
        }

        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => {
                    self.current_fg = None;
                    self.current_bg = None;
                }
                1 | 22 => {} // Bold on/off - ignored
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
            };
        }
        self.cursor_x += 1;
    }

    fn line_feed(&mut self) {
        if self.cursor_y + 1 >= self.height {
            // Scroll up
            self.scroll_up();
        } else {
            self.cursor_y += 1;
        }
    }

    fn scroll_up(&mut self) {
        // Move all lines up, discard top line
        let width = self.width as usize;
        for y in 0..(self.height as usize - 1) {
            let src = (y + 1) * width;
            let dst = y * width;
            for x in 0..width {
                self.cells[dst + x] = self.cells[src + x];
            }
        }
        // Clear bottom line
        let blank = self.blank_cell();
        let last_row = (self.height as usize - 1) * width;
        for x in 0..width {
            self.cells[last_row + x] = blank;
        }
    }

    fn insert_lines(&mut self, n: usize) {
        let y = self.cursor_y as usize;
        let width = self.width as usize;
        let height = self.height as usize;

        // Move lines down
        for row in (y..height.saturating_sub(n)).rev() {
            for x in 0..width {
                self.cells[(row + n) * width + x] = self.cells[row * width + x];
            }
        }
        // Clear inserted lines
        let blank = self.blank_cell();
        for row in y..(y + n).min(height) {
            for x in 0..width {
                self.cells[row * width + x] = blank;
            }
        }
    }

    fn delete_lines(&mut self, n: usize) {
        let y = self.cursor_y as usize;
        let width = self.width as usize;
        let height = self.height as usize;

        // Move lines up
        for row in y..height.saturating_sub(n) {
            for x in 0..width {
                self.cells[row * width + x] = self.cells[(row + n) * width + x];
            }
        }
        // Clear bottom lines
        let blank = self.blank_cell();
        for row in height.saturating_sub(n)..height {
            for x in 0..width {
                self.cells[row * width + x] = blank;
            }
        }
    }

    /// Create a blank cell with current background color (for erase operations)
    fn blank_cell(&self) -> Cell {
        Cell {
            ch: ' ',
            fg: None,
            bg: self.current_bg,
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
        for y in (self.cursor_y as usize + 1)..(self.height as usize) {
            for x in 0..width {
                self.cells[y * width + x] = blank;
            }
        }
    }

    fn erase_above(&mut self) {
        // Erase from start to cursor
        self.erase_line_left();
        let blank = self.blank_cell();
        let width = self.width as usize;
        for y in 0..(self.cursor_y as usize) {
            for x in 0..width {
                self.cells[y * width + x] = blank;
            }
        }
    }

    fn erase_line(&mut self) {
        let blank = self.blank_cell();
        let y = self.cursor_y as usize;
        let width = self.width as usize;
        for x in 0..width {
            self.cells[y * width + x] = blank;
        }
    }

    fn erase_line_right(&mut self) {
        let blank = self.blank_cell();
        let y = self.cursor_y as usize;
        let width = self.width as usize;
        for x in (self.cursor_x as usize)..width {
            self.cells[y * width + x] = blank;
        }
    }

    fn erase_line_left(&mut self) {
        let blank = self.blank_cell();
        let y = self.cursor_y as usize;
        let width = self.width as usize;
        for x in 0..=(self.cursor_x as usize).min(width - 1) {
            self.cells[y * width + x] = blank;
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

/// Compositor renders all pane buffers to the terminal
pub struct Compositor {
    width: u16,
    height: u16,
    // Track what's currently on screen to minimize updates
    last_frame: Vec<Cell>,
}

impl Compositor {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            last_frame: vec![Cell::default(); (width as usize) * (height as usize)],
        }
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.last_frame = vec![Cell::default(); (width as usize) * (height as usize)];
    }

    /// Render a pane's buffer to the given rect
    pub fn render_pane<W: Write>(
        &mut self,
        writer: &mut W,
        buffer: &ScreenBuffer,
        rect: Rect,
        focused: bool,
    ) -> std::io::Result<()> {
        use crossterm::style::{Attribute, SetAttribute};

        for y in 0..rect.height.min(buffer.height()) {
            queue!(writer, MoveTo(rect.x, rect.y + y), ResetColor)?;

            // Dim unfocused panes
            if !focused {
                queue!(writer, SetAttribute(Attribute::Dim))?;
            }

            let mut last_fg: Option<Color> = None;
            let mut last_bg: Option<Color> = None;

            for x in 0..rect.width.min(buffer.width()) {
                let cell = buffer.get(x, y);

                // Only emit color changes when needed
                let need_fg_change = cell.fg != last_fg;
                let need_bg_change = cell.bg != last_bg;

                if need_fg_change || need_bg_change {
                    queue!(writer, ResetColor)?;
                    if !focused {
                        queue!(writer, SetAttribute(Attribute::Dim))?;
                    }
                    if let Some(fg) = cell.fg {
                        queue!(writer, SetForegroundColor(fg))?;
                    }
                    if let Some(bg) = cell.bg {
                        queue!(writer, SetBackgroundColor(bg))?;
                    }
                    last_fg = cell.fg;
                    last_bg = cell.bg;
                }

                write!(writer, "{}", cell.ch)?;
            }

            queue!(writer, ResetColor, SetAttribute(Attribute::Reset))?;
        }

        // Position cursor in focused pane
        if focused {
            let (cx, cy) = buffer.cursor();
            queue!(writer, MoveTo(rect.x + cx, rect.y + cy))?;
        }

        Ok(())
    }
}
