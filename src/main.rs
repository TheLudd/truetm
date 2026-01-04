//! dvtr - A truecolor-enabled terminal multiplexer inspired by dvtm

mod layout;
mod pane;
mod render;
mod tag;

use anyhow::{Context, Result};
use crossterm::{
    cursor::MoveTo,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute, queue,
    style::ResetColor,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use layout::LayoutManager;
use pane::{Pane, PaneId, PaneManager, PtyMessage, Rect};
use render::{Compositor, ScreenBuffer};
use tag::TagSet;
use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

fn main() -> Result<()> {
    env_logger::init();

    let result = run();

    // Cleanup
    let _ = terminal::disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, ResetColor);

    result
}

/// Pending commands that need a second keypress
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingCommand {
    ViewTag,    // 'v' - waiting for tag number
    SetTag,     // 't' - waiting for tag number
    ToggleTag,  // 'T' - waiting for tag number
}

/// Application state
struct App {
    panes: PaneManager,
    buffers: HashMap<PaneId, ScreenBuffer>,
    layout: LayoutManager,
    compositor: Compositor,
    pty_tx: Sender<PtyMessage>,
    pty_rx: Receiver<PtyMessage>,
    shell: String,
    env_vars: Vec<(String, String)>,
    width: u16,
    height: u16,
    prefix_mode: bool,
    pending_command: Option<PendingCommand>,
    needs_redraw: bool,
    running: bool,
    // Tagging
    current_view: TagSet,
    tag_count: u8,
}

impl App {
    fn new(width: u16, height: u16) -> Self {
        let (pty_tx, pty_rx) = mpsc::channel();

        // Collect environment variables
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut env_vars = vec![
            ("TERM".to_string(), std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".to_string())),
            ("COLORTERM".to_string(), "truecolor".to_string()),
        ];

        for var in ["HOME", "USER", "SHELL", "PATH", "LANG", "LC_ALL", "LC_CTYPE",
                    "XDG_RUNTIME_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME",
                    "EDITOR", "VISUAL", "PAGER"] {
            if let Ok(val) = std::env::var(var) {
                env_vars.push((var.to_string(), val));
            }
        }

        Self {
            panes: PaneManager::new(),
            buffers: HashMap::new(),
            layout: LayoutManager::new(),
            compositor: Compositor::new(width, height - 1), // -1 for status bar
            pty_tx,
            pty_rx,
            shell,
            env_vars,
            width,
            height,
            prefix_mode: false,
            pending_command: None,
            needs_redraw: true,
            running: true,
            current_view: TagSet::single(0), // Start on tag 1 (index 0)
            tag_count: 9,
        }
    }

    /// Create a new pane with current view's tags
    fn create_pane(&mut self) -> Result<PaneId> {
        self.create_pane_with_tags(self.current_view)
    }

    /// Create a new pane with specific tags
    fn create_pane_with_tags(&mut self, tags: TagSet) -> Result<PaneId> {
        let id = self.panes.next_id();

        // Initial rect - will be updated by layout
        let content_height = self.height.saturating_sub(1); // Reserve 1 row for status bar
        let rect = Rect::new(0, 0, self.width, content_height);

        // Buffer height is rect.height - 1 to reserve header row
        let buffer_height = rect.height.saturating_sub(1);
        let pane = Pane::new_with_size(id, rect, tags, &self.shell, &self.env_vars, self.pty_tx.clone(), rect.width, buffer_height)?;
        // add() inserts at front (master) and focuses
        self.panes.add(pane);
        self.buffers.insert(id, ScreenBuffer::new(rect.width, buffer_height));

        self.apply_layout()?;
        self.needs_redraw = true;

        Ok(id)
    }

    /// Close the focused pane
    fn close_focused_pane(&mut self) {
        if let Some(pane) = self.panes.focused() {
            let id = pane.id;
            self.panes.remove(id);
            self.buffers.remove(&id);
            if let Err(e) = self.apply_layout() {
                log::error!("Failed to apply layout after closing pane: {}", e);
            }
            self.needs_redraw = true;
        }
    }

    /// Apply the current layout to visible panes
    fn apply_layout(&mut self) -> Result<()> {
        // Only layout panes visible in current view
        let pane_ids = self.panes.visible_in_view(self.current_view);
        // Content area excludes status bar (1 row at bottom)
        let content_height = self.height.saturating_sub(1);
        let area = Rect::new(0, 0, self.width, content_height);
        let positions = self.layout.arrange(&pane_ids, area);

        for (pane_id, rect) in positions {
            // Buffer/PTY height is rect.height - 1 to reserve header row
            let buffer_height = rect.height.saturating_sub(1);
            if let Some(pane) = self.panes.get_mut(pane_id) {
                pane.set_rect_with_size(rect, rect.width, buffer_height)?;
            }
            if let Some(buffer) = self.buffers.get_mut(&pane_id) {
                buffer.resize(rect.width, buffer_height);
            }
        }

        // Ensure focus is on a visible pane
        self.panes.ensure_focus_in_view(self.current_view);

        Ok(())
    }

    /// Handle resize
    fn resize(&mut self, width: u16, height: u16) -> Result<()> {
        self.width = width;
        self.height = height;
        self.compositor.resize(width, height.saturating_sub(1)); // -1 for status bar
        self.apply_layout()?;
        self.needs_redraw = true;
        Ok(())
    }

    /// Process PTY output
    fn process_pty_messages(&mut self) {
        while let Ok(msg) = self.pty_rx.try_recv() {
            match msg {
                PtyMessage::Data { pane_id, data } => {
                    if let Some(buffer) = self.buffers.get_mut(&pane_id) {
                        buffer.process(&data);
                        self.needs_redraw = true;
                    }
                }
                PtyMessage::Exit { pane_id } => {
                    self.panes.mark_exited(pane_id);
                }
            }
        }

        // Clean up exited panes
        let had_exited = self.panes.all().iter().any(|p| p.exited);
        if had_exited {
            let exited_ids: Vec<_> = self.panes.all().iter().filter(|p| p.exited).map(|p| p.id).collect();
            for id in exited_ids {
                self.buffers.remove(&id);
            }
            self.panes.remove_exited();
            if let Err(e) = self.apply_layout() {
                log::error!("Failed to apply layout after removing exited panes: {}", e);
            }
            self.needs_redraw = true;
        }
    }

    /// Handle keyboard input
    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Check for prefix key: Ctrl+B
        if !self.prefix_mode
            && self.pending_command.is_none()
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('b')
        {
            self.prefix_mode = true;
            return Ok(());
        }

        // Handle pending command (waiting for a number after v/t/T)
        if let Some(pending) = self.pending_command {
            self.pending_command = None;

            if let KeyCode::Char(c) = key.code {
                if let Some(num) = c.to_digit(10) {
                    match pending {
                        PendingCommand::ViewTag => {
                            if num >= 1 && num <= 9 {
                                let tag = (num - 1) as u8;
                                self.current_view = TagSet::single(tag);
                                // Auto-create pane if tag is empty
                                if self.panes.visible_in_view(self.current_view).is_empty() {
                                    self.create_pane()?;
                                }
                                self.apply_layout()?;
                                self.needs_redraw = true;
                            } else if num == 0 {
                                // View all tags
                                self.current_view = TagSet::ALL;
                                self.apply_layout()?;
                                self.needs_redraw = true;
                            }
                        }
                        PendingCommand::SetTag => {
                            if num >= 1 && num <= 9 {
                                let tag = (num - 1) as u8;
                                if let Some(pane) = self.panes.focused_mut() {
                                    pane.tags = TagSet::single(tag);
                                }
                                self.apply_layout()?;
                                self.needs_redraw = true;
                            }
                        }
                        PendingCommand::ToggleTag => {
                            if num >= 1 && num <= 9 {
                                let tag = (num - 1) as u8;
                                if let Some(pane) = self.panes.focused_mut() {
                                    pane.tags.toggle(tag);
                                    // Ensure pane has at least one tag
                                    if pane.tags.is_empty() {
                                        pane.tags = TagSet::single(tag);
                                    }
                                }
                                self.apply_layout()?;
                                self.needs_redraw = true;
                            }
                        }
                    }
                }
            }
            return Ok(());
        }

        if self.prefix_mode {
            self.prefix_mode = false;

            // Check for window focus (1-9 focuses visible window N)
            if let KeyCode::Char(c) = key.code {
                if let Some(num) = c.to_digit(10) {
                    if num >= 1 && num <= 9 {
                        // Focus visible window N (1-indexed)
                        let visible = self.panes.visible_in_view(self.current_view);
                        let idx = (num - 1) as usize;
                        if idx < visible.len() {
                            self.panes.focus_by_id(visible[idx]);
                            self.needs_redraw = true;
                        }
                        return Ok(());
                    }
                }
            }

            match key.code {
                KeyCode::Char('q') => {
                    self.running = false;
                }
                KeyCode::Char('c') => {
                    // Create new pane
                    self.create_pane()?;
                }
                KeyCode::Char('x') => {
                    // Close focused pane
                    self.close_focused_pane();
                    if self.panes.is_empty() {
                        self.running = false;
                    }
                }
                KeyCode::Char('j') => {
                    // Focus next (within current view)
                    self.panes.focus_next_in_view(self.current_view);
                    self.needs_redraw = true;
                }
                KeyCode::Char('k') => {
                    // Focus previous (within current view)
                    self.panes.focus_prev_in_view(self.current_view);
                    self.needs_redraw = true;
                }
                KeyCode::Char(' ') => {
                    // Next layout
                    self.layout.next();
                    self.apply_layout()?;
                    self.needs_redraw = true;
                }
                KeyCode::Char('h') => {
                    // Decrease master size
                    self.layout.adjust_master(-0.05);
                    self.apply_layout()?;
                    self.needs_redraw = true;
                }
                KeyCode::Char('l') => {
                    // Increase master size
                    self.layout.adjust_master(0.05);
                    self.apply_layout()?;
                    self.needs_redraw = true;
                }
                KeyCode::Char('b') => {
                    // Send literal Ctrl+B
                    if let Some(pane) = self.panes.focused_mut() {
                        pane.write(&[0x02])?;
                    }
                }
                KeyCode::Enter => {
                    // Swap focused pane with master
                    self.panes.swap_with_master(self.current_view);
                    self.apply_layout()?;
                    self.needs_redraw = true;
                }
                KeyCode::Char('v') => {
                    // View tag: wait for number
                    self.pending_command = Some(PendingCommand::ViewTag);
                }
                KeyCode::Char('t') => {
                    // Set tag: wait for number
                    self.pending_command = Some(PendingCommand::SetTag);
                }
                KeyCode::Char('T') => {
                    // Toggle tag: wait for number
                    self.pending_command = Some(PendingCommand::ToggleTag);
                }
                _ => {}
            }
            return Ok(());
        }

        // Forward to focused pane
        if let Some(pane) = self.panes.focused_mut() {
            let bytes = key_event_to_bytes(&key);
            pane.write(&bytes)?;
        }

        Ok(())
    }

    /// Render the screen
    fn render(&mut self) -> Result<()> {
        if !self.needs_redraw {
            return Ok(());
        }

        let mut stdout = io::stdout();

        // Clear screen
        queue!(stdout, Clear(ClearType::All))?;

        // Get visible panes and focused pane
        let visible_ids = self.panes.visible_in_view(self.current_view);
        let focused_id = self.panes.focused().map(|p| p.id);

        // Render only visible panes with window numbers
        for (win_num, &pane_id) in visible_ids.iter().enumerate() {
            if let Some(pane) = self.panes.get(pane_id) {
                if let Some(buffer) = self.buffers.get(&pane.id) {
                    let is_focused = Some(pane.id) == focused_id;
                    // Content rect starts at y+1 to leave room for header
                    let content_rect = Rect::new(
                        pane.rect.x,
                        pane.rect.y + 1,
                        pane.rect.width,
                        pane.rect.height.saturating_sub(1),
                    );
                    self.compositor.render_pane(&mut stdout, buffer, content_rect, is_focused)?;
                    // Draw window number in header row
                    self.draw_window_number(&mut stdout, pane.rect, win_num + 1, is_focused)?;
                }
            }
        }

        // Draw separators between visible panes
        let content_height = self.height.saturating_sub(1);
        if visible_ids.len() > 1 && self.layout.current_name() == "vertical" {
            // Find the master pane (first visible)
            if let Some(&master_id) = visible_ids.first() {
                if let Some(master) = self.panes.get(master_id) {
                    let sep_x = master.rect.x + master.rect.width;
                    if sep_x < self.width {
                        queue!(stdout, ResetColor)?;
                        for y in 0..content_height {
                            queue!(stdout, MoveTo(sep_x, y))?;
                            write!(stdout, "│")?;
                        }
                    }
                }
            }
        }

        // Render status bar at bottom
        self.render_status_bar(&mut stdout)?;

        // Position cursor in focused pane (if visible)
        if let Some(pane) = self.panes.focused() {
            if visible_ids.contains(&pane.id) {
                if let Some(buffer) = self.buffers.get(&pane.id) {
                    let (cx, cy) = buffer.cursor();
                    // +1 to account for header row
                    queue!(stdout, MoveTo(pane.rect.x + cx, pane.rect.y + 1 + cy))?;
                }
            }
        }

        stdout.flush()?;
        self.needs_redraw = false;

        Ok(())
    }

    /// Render the status bar showing tags
    fn render_status_bar(&self, stdout: &mut impl Write) -> Result<()> {
        use crossterm::style::{Attribute, SetAttribute, SetForegroundColor, Color};

        let status_y = self.height.saturating_sub(1);
        queue!(stdout, MoveTo(0, status_y), ResetColor)?;

        // Clear the status line
        write!(stdout, "{:width$}", "", width = self.width as usize)?;
        queue!(stdout, MoveTo(0, status_y))?;

        // Render only tags that have panes or are currently viewed
        for tag in 0..self.tag_count {
            let has_panes = self.panes.any_with_tag(tag);
            let is_viewed = self.current_view.contains(tag);

            // Skip tags with no panes and not currently viewed
            if !has_panes && !is_viewed {
                continue;
            }

            let is_focused_tag = self.panes.focused()
                .map(|p| p.tags.contains(tag))
                .unwrap_or(false);

            // Style based on state - green theme
            if is_viewed && is_focused_tag {
                // Viewed and focused pane has this tag - bright green bold
                queue!(stdout, SetForegroundColor(Color::Green), SetAttribute(Attribute::Bold))?;
            } else if is_viewed {
                // Currently viewing this tag - green
                queue!(stdout, SetForegroundColor(Color::Green))?;
            } else {
                // Has panes but not viewing - dark green
                queue!(stdout, SetForegroundColor(Color::DarkGreen))?;
            }

            write!(stdout, "[{}]", tag + 1)?;
            queue!(stdout, ResetColor, SetAttribute(Attribute::Reset))?;
            write!(stdout, " ")?;
        }

        // Show layout name
        queue!(stdout, SetForegroundColor(Color::DarkGrey))?;
        write!(stdout, "[{}]", self.layout.current_name())?;
        queue!(stdout, ResetColor)?;

        Ok(())
    }

    /// Draw window header with number and line
    fn draw_window_number(&self, stdout: &mut impl Write, rect: Rect, num: usize, is_focused: bool) -> Result<()> {
        use crossterm::style::{SetForegroundColor, Color, Attribute, SetAttribute};

        queue!(stdout, MoveTo(rect.x, rect.y))?;

        if is_focused {
            queue!(stdout, SetForegroundColor(Color::Green), SetAttribute(Attribute::Bold))?;
        } else {
            queue!(stdout, SetForegroundColor(Color::DarkGrey))?;
        }

        // Draw window number
        let num_str = format!("[{}]", num);
        write!(stdout, "{}", num_str)?;

        // Draw line for the rest of the header
        let line_width = rect.width.saturating_sub(num_str.len() as u16);
        for _ in 0..line_width {
            write!(stdout, "─")?;
        }

        queue!(stdout, ResetColor, SetAttribute(Attribute::Reset))?;

        Ok(())
    }
}

fn run() -> Result<()> {
    let (width, height) = terminal::size().context("Failed to get terminal size")?;

    let mut app = App::new(width, height);

    // Create initial pane
    app.create_pane()?;

    // Set up terminal
    terminal::enable_raw_mode().context("Failed to enable raw mode")?;
    execute!(io::stdout(), EnterAlternateScreen)?;

    // Main loop
    while app.running {
        // Process PTY output
        app.process_pty_messages();

        // Check for input
        if event::poll(Duration::from_millis(10))? {
            match event::read()? {
                Event::Key(key) => app.handle_key(key)?,
                Event::Resize(w, h) => app.resize(w, h)?,
                _ => {}
            }
        }

        // Render
        app.render()?;

        // Exit if no panes left
        if app.panes.is_empty() {
            app.running = false;
        }
    }

    Ok(())
}

/// Convert a crossterm KeyEvent to bytes to send to PTY
fn key_event_to_bytes(key: &KeyEvent) -> Vec<u8> {
    let mut bytes = Vec::new();

    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                if c.is_ascii_lowercase() {
                    bytes.push((c as u8) - b'a' + 1);
                } else if c.is_ascii_uppercase() {
                    bytes.push((c.to_ascii_lowercase() as u8) - b'a' + 1);
                } else {
                    match c {
                        '[' => bytes.push(0x1b),
                        '\\' => bytes.push(0x1c),
                        ']' => bytes.push(0x1d),
                        '^' => bytes.push(0x1e),
                        '_' => bytes.push(0x1f),
                        _ => bytes.extend(c.to_string().as_bytes()),
                    }
                }
            } else if key.modifiers.contains(KeyModifiers::ALT) {
                bytes.push(0x1b);
                bytes.extend(c.to_string().as_bytes());
            } else {
                bytes.extend(c.to_string().as_bytes());
            }
        }
        KeyCode::Enter => bytes.push(b'\r'),
        KeyCode::Tab => bytes.push(b'\t'),
        KeyCode::Backspace => bytes.push(0x7f),
        KeyCode::Esc => bytes.push(0x1b),
        KeyCode::Up => bytes.extend(b"\x1b[A"),
        KeyCode::Down => bytes.extend(b"\x1b[B"),
        KeyCode::Right => bytes.extend(b"\x1b[C"),
        KeyCode::Left => bytes.extend(b"\x1b[D"),
        KeyCode::Home => bytes.extend(b"\x1b[H"),
        KeyCode::End => bytes.extend(b"\x1b[F"),
        KeyCode::PageUp => bytes.extend(b"\x1b[5~"),
        KeyCode::PageDown => bytes.extend(b"\x1b[6~"),
        KeyCode::Insert => bytes.extend(b"\x1b[2~"),
        KeyCode::Delete => bytes.extend(b"\x1b[3~"),
        KeyCode::F(n) => {
            let seq = match n {
                1 => b"\x1bOP".to_vec(),
                2 => b"\x1bOQ".to_vec(),
                3 => b"\x1bOR".to_vec(),
                4 => b"\x1bOS".to_vec(),
                5 => b"\x1b[15~".to_vec(),
                6 => b"\x1b[17~".to_vec(),
                7 => b"\x1b[18~".to_vec(),
                8 => b"\x1b[19~".to_vec(),
                9 => b"\x1b[20~".to_vec(),
                10 => b"\x1b[21~".to_vec(),
                11 => b"\x1b[23~".to_vec(),
                12 => b"\x1b[24~".to_vec(),
                _ => Vec::new(),
            };
            bytes.extend(seq);
        }
        _ => {}
    }

    bytes
}
