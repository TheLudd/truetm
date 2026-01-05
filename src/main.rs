//! dvtr - A truecolor-enabled terminal multiplexer inspired by dvtm

mod config;
mod layout;
mod pane;
mod render;
mod tag;

use anyhow::{Context, Result};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind, MouseButton,
        KeyboardEnhancementFlags, PushKeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        EnableMouseCapture, DisableMouseCapture,
    },
    execute, queue,
    style::ResetColor,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen, SetTitle},
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
    let _ = execute!(io::stdout(), DisableMouseCapture);
    let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
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

/// Mouse selection state
#[derive(Debug, Clone)]
struct MouseSelection {
    pane_id: PaneId,
    // Buffer coordinates (relative to pane content area)
    buf_start_x: u16,
    buf_start_y: u16,
    buf_end_x: u16,
    buf_end_y: u16,
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
    // Tag history stack (unique - each tag appears once, most recent at end)
    tag_history: Vec<u8>,
    // Broadcast mode - send input to all visible panes
    broadcast_mode: bool,
    // Scroll mode - viewing scrollback
    scroll_mode: bool,
    scroll_offset: usize,
    // Zoom mode - focused pane is fullscreen
    zoomed_pane: Option<PaneId>,
    // Mouse selection
    mouse_selection: Option<MouseSelection>,
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
            tag_history: vec![0], // Start with tag 0 in history
            broadcast_mode: false,
            scroll_mode: false,
            scroll_offset: 0,
            zoomed_pane: None,
            mouse_selection: None,
        }
    }

    /// Switch to a tag, updating history (unique stack - moves tag to top if already present)
    fn switch_to_tag(&mut self, tag: u8) {
        // Remove tag from history if present (to avoid duplicates)
        self.tag_history.retain(|&t| t != tag);
        // Push to top
        self.tag_history.push(tag);
        // Update view
        self.current_view = TagSet::single(tag);
        // Force full redraw since visible panes changed
        self.compositor.invalidate();
    }

    /// Go to previous tag in history (pop current, switch to new top)
    fn go_to_previous_tag(&mut self) {
        if self.tag_history.len() > 1 {
            // Pop current tag
            self.tag_history.pop();
            // Switch to new top
            if let Some(&tag) = self.tag_history.last() {
                self.current_view = TagSet::single(tag);
            }
            // Force full redraw since visible panes changed
            self.compositor.invalidate();
        }
    }

    /// Create a new pane with current view's tags, inheriting cwd from focused pane
    fn create_pane(&mut self) -> Result<PaneId> {
        // Get cwd from focused pane, or use current directory for initial pane
        let cwd = self.panes.focused()
            .and_then(|p| p.get_cwd())
            .or_else(|| std::env::current_dir().ok());
        self.create_pane_with_tags(self.current_view, cwd)
    }

    /// Create a new pane with specific tags and optional working directory
    fn create_pane_with_tags(&mut self, tags: TagSet, cwd: Option<std::path::PathBuf>) -> Result<PaneId> {
        let id = self.panes.next_id();

        // Initial rect - will be updated by layout
        let content_height = self.height.saturating_sub(1); // Reserve 1 row for status bar
        let rect = Rect::new(0, 0, self.width, content_height);

        // Buffer height is rect.height - 1 to reserve header row
        let buffer_height = rect.height.saturating_sub(1);
        let pane = Pane::new_with_size(id, rect, tags, &self.shell, &self.env_vars, cwd, self.pty_tx.clone(), rect.width, buffer_height)?;
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

            // Exit zoom mode if closing zoomed pane
            if self.zoomed_pane == Some(id) {
                self.zoomed_pane = None;
            }

            self.panes.remove(id);
            self.buffers.remove(&id);

            // If current tag is now empty, go to previous tag in history
            if self.panes.visible_in_view(self.current_view).is_empty() {
                self.go_to_previous_tag();
            }

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

        // In zoom mode, only show the zoomed pane
        if let Some(zoomed_id) = self.zoomed_pane {
            // Check if zoomed pane is still visible in current view
            if pane_ids.contains(&zoomed_id) {
                // Give zoomed pane the full area
                let buffer_height = area.height.saturating_sub(1);
                if let Some(pane) = self.panes.get_mut(zoomed_id) {
                    pane.set_rect_with_size(area, area.width, buffer_height)?;
                }
                if let Some(buffer) = self.buffers.get_mut(&zoomed_id) {
                    buffer.resize(area.width, buffer_height);
                }
                // Hide all other panes
                for &pane_id in &pane_ids {
                    if pane_id != zoomed_id {
                        if let Some(pane) = self.panes.get_mut(pane_id) {
                            pane.rect = Rect::new(0, 0, 0, 0);
                        }
                    }
                }
            } else {
                // Zoomed pane no longer visible, exit zoom mode
                self.zoomed_pane = None;
                return self.apply_layout(); // Re-apply without zoom
            }
        } else {
            // Normal layout
            let positions = self.layout.arrange(&pane_ids, area);

            // Track which panes are in the layout
            let layout_pane_ids: Vec<_> = positions.iter().map(|(id, _)| *id).collect();

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

            // Hide panes not in layout (e.g., in monocle mode)
            for &pane_id in &pane_ids {
                if !layout_pane_ids.contains(&pane_id) {
                    if let Some(pane) = self.panes.get_mut(pane_id) {
                        pane.rect = Rect::new(0, 0, 0, 0);
                    }
                }
            }
        }

        // Ensure focus is on a visible pane
        self.panes.ensure_focus_in_view(self.current_view);

        // Force full redraw when layout changes
        self.compositor.invalidate();

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

    /// Process PTY output, returns true if any data was processed
    fn process_pty_messages(&mut self) -> bool {
        let mut had_data = false;

        while let Ok(msg) = self.pty_rx.try_recv() {
            match msg {
                PtyMessage::Data { pane_id, data } => {
                    if let Some(buffer) = self.buffers.get_mut(&pane_id) {
                        buffer.process(&data);
                        self.needs_redraw = true;
                        had_data = true;

                        // Send any terminal responses back to the PTY
                        let responses = buffer.drain_responses();
                        if !responses.is_empty() {
                            if let Some(pane) = self.panes.get_mut(pane_id) {
                                for response in responses {
                                    let _ = pane.write(&response);
                                }
                            }
                        }
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

            // If current tag is now empty, go to previous tag in history
            if self.panes.visible_in_view(self.current_view).is_empty() {
                self.go_to_previous_tag();
            }

            if let Err(e) = self.apply_layout() {
                log::error!("Failed to apply layout after removing exited panes: {}", e);
            }
            self.needs_redraw = true;
        }

        had_data
    }

    /// Handle keyboard input
    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Check for prefix key
        if !self.prefix_mode
            && self.pending_command.is_none()
            && key.modifiers.contains(config::PREFIX_MODIFIERS)
            && key.code == config::PREFIX_KEY
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
                                self.switch_to_tag(tag);
                                // Auto-create pane if tag is empty
                                if self.panes.visible_in_view(self.current_view).is_empty() {
                                    self.create_pane()?;
                                }
                                self.apply_layout()?;
                                self.needs_redraw = true;
                            } else if num == 0 {
                                // View all tags (doesn't affect history)
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
                k if k == config::KEY_QUIT => {
                    self.running = false;
                }
                k if k == config::KEY_NEW_WINDOW => {
                    self.create_pane()?;
                }
                k if k == config::KEY_CLOSE_WINDOW => {
                    self.close_focused_pane();
                    if self.panes.is_empty() {
                        self.running = false;
                    }
                }
                k if k == config::KEY_FOCUS_NEXT => {
                    self.panes.focus_next_in_view(self.current_view);
                    self.needs_redraw = true;
                }
                k if k == config::KEY_FOCUS_PREV => {
                    self.panes.focus_prev_in_view(self.current_view);
                    self.needs_redraw = true;
                }
                k if k == config::KEY_MASTER_SHRINK => {
                    self.layout.adjust_master(-config::MASTER_ADJUST_STEP);
                    self.apply_layout()?;
                    self.needs_redraw = true;
                }
                k if k == config::KEY_MASTER_GROW => {
                    self.layout.adjust_master(config::MASTER_ADJUST_STEP);
                    self.apply_layout()?;
                    self.needs_redraw = true;
                }
                k if k == config::PREFIX_KEY => {
                    // Send literal prefix key (Ctrl+letter = letter - 'a' + 1)
                    if let KeyCode::Char(c) = config::PREFIX_KEY {
                        let ctrl_byte = (c.to_ascii_lowercase() as u8) - b'a' + 1;
                        if let Some(pane) = self.panes.focused_mut() {
                            pane.write(&[ctrl_byte])?;
                        }
                    }
                }
                k if k == config::KEY_SWAP_MASTER => {
                    self.panes.swap_with_master(self.current_view);
                    self.apply_layout()?;
                    self.needs_redraw = true;
                }
                k if k == config::KEY_VIEW_TAG => {
                    self.pending_command = Some(PendingCommand::ViewTag);
                }
                k if k == config::KEY_SET_TAG => {
                    self.pending_command = Some(PendingCommand::SetTag);
                }
                k if k == config::KEY_TOGGLE_TAG => {
                    self.pending_command = Some(PendingCommand::ToggleTag);
                }
                k if k == config::KEY_TOGGLE_BROADCAST => {
                    self.broadcast_mode = !self.broadcast_mode;
                    self.needs_redraw = true;
                }
                k if k == config::KEY_ENTER_SCROLL => {
                    self.scroll_mode = true;
                    self.scroll_offset = 0;
                    self.compositor.invalidate();
                    self.needs_redraw = true;
                }
                k if k == config::KEY_ZOOM => {
                    if let Some(focused) = self.panes.focused() {
                        let focused_id = focused.id;
                        if self.zoomed_pane == Some(focused_id) {
                            // Already zoomed on this pane, unzoom
                            self.zoomed_pane = None;
                        } else {
                            // Zoom on focused pane
                            self.zoomed_pane = Some(focused_id);
                        }
                        self.apply_layout()?;
                        self.needs_redraw = true;
                    }
                }
                _ => {}
            }
            return Ok(());
        }

        // Handle scroll mode
        if self.scroll_mode {
            match key.code {
                k if k == config::SCROLL_EXIT_1 || k == config::SCROLL_EXIT_2 => {
                    self.scroll_mode = false;
                    self.scroll_offset = 0;
                    self.compositor.invalidate();
                    self.needs_redraw = true;
                }
                k if k == config::SCROLL_DOWN_LINE_1 || k == config::SCROLL_DOWN_LINE_2 => {
                    if self.scroll_offset > 0 {
                        self.scroll_offset -= 1;
                        self.compositor.invalidate();
                        self.needs_redraw = true;
                    }
                }
                k if k == config::SCROLL_UP_LINE_1 || k == config::SCROLL_UP_LINE_2 => {
                    if let Some(pane) = self.panes.focused() {
                        if let Some(buffer) = self.buffers.get(&pane.id) {
                            let max_offset = buffer.scrollback_len();
                            if self.scroll_offset < max_offset {
                                self.scroll_offset += 1;
                                self.compositor.invalidate();
                                self.needs_redraw = true;
                            }
                        }
                    }
                }
                k if k == config::SCROLL_UP_PAGE => {
                    if let Some(pane) = self.panes.focused() {
                        if let Some(buffer) = self.buffers.get(&pane.id) {
                            let max_offset = buffer.scrollback_len();
                            let page_size = (buffer.height() / 2).max(1) as usize;
                            self.scroll_offset = (self.scroll_offset + page_size).min(max_offset);
                            self.compositor.invalidate();
                            self.needs_redraw = true;
                        }
                    }
                }
                k if k == config::SCROLL_DOWN_PAGE => {
                    if let Some(pane) = self.panes.focused() {
                        if let Some(buffer) = self.buffers.get(&pane.id) {
                            let page_size = (buffer.height() / 2).max(1) as usize;
                            self.scroll_offset = self.scroll_offset.saturating_sub(page_size);
                            self.compositor.invalidate();
                            self.needs_redraw = true;
                        }
                    }
                }
                k if k == config::SCROLL_TO_TOP => {
                    if let Some(pane) = self.panes.focused() {
                        if let Some(buffer) = self.buffers.get(&pane.id) {
                            self.scroll_offset = buffer.scrollback_len();
                            self.compositor.invalidate();
                            self.needs_redraw = true;
                        }
                    }
                }
                k if k == config::SCROLL_TO_BOTTOM => {
                    self.scroll_offset = 0;
                    self.compositor.invalidate();
                    self.needs_redraw = true;
                }
                _ => {}
            }
            return Ok(());
        }

        // Forward input to pane(s)
        let bytes = key_event_to_bytes(&key);
        if self.broadcast_mode {
            // Send to all visible panes
            let visible_ids = self.panes.visible_in_view(self.current_view);
            for id in visible_ids {
                if let Some(pane) = self.panes.get_mut(id) {
                    pane.write(&bytes)?;
                }
            }
        } else {
            // Send to focused pane only
            if let Some(pane) = self.panes.focused_mut() {
                pane.write(&bytes)?;
            }
        }

        Ok(())
    }

    /// Handle mouse input
    fn handle_mouse(&mut self, mouse: MouseEvent) -> Result<()> {
        let x = mouse.column;
        let y = mouse.row;

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Find which pane was clicked
                if let Some((pane_id, buf_x, buf_y)) = self.pane_at_position(x, y) {
                    // Clear old selection by invalidating compositor
                    if self.mouse_selection.is_some() {
                        self.compositor.invalidate();
                    }
                    // Start selection
                    self.mouse_selection = Some(MouseSelection {
                        pane_id,
                        buf_start_x: buf_x,
                        buf_start_y: buf_y,
                        buf_end_x: buf_x,
                        buf_end_y: buf_y,
                    });
                    // Focus the clicked pane
                    self.panes.focus_by_id(pane_id);
                    self.needs_redraw = true;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // Extend selection - check position first, then update selection
                let pos_info = self.pane_at_position(x, y);
                if let Some(ref mut sel) = self.mouse_selection {
                    if let Some((pane_id, buf_x, buf_y)) = pos_info {
                        if pane_id == sel.pane_id {
                            sel.buf_end_x = buf_x;
                            sel.buf_end_y = buf_y;
                            self.needs_redraw = true;
                        }
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                // Finish selection and copy to clipboard (but keep selection visible)
                if let Some(ref sel) = self.mouse_selection {
                    if sel.buf_start_x != sel.buf_end_x || sel.buf_start_y != sel.buf_end_y {
                        // Extract selected text and copy
                        if let Some(text) = self.extract_selection(sel) {
                            self.copy_to_clipboard(&text)?;
                        }
                    }
                }
            }
            MouseEventKind::ScrollUp => {
                // Scroll focused pane up (into history)
                if !self.scroll_mode {
                    self.scroll_mode = true;
                    self.scroll_offset = 0;
                }
                if let Some(pane) = self.panes.focused() {
                    if let Some(buffer) = self.buffers.get(&pane.id) {
                        let max_offset = buffer.scrollback_len();
                        if self.scroll_offset < max_offset {
                            self.scroll_offset += 3;
                            self.scroll_offset = self.scroll_offset.min(max_offset);
                            self.compositor.invalidate();
                            self.needs_redraw = true;
                        }
                    }
                }
            }
            MouseEventKind::ScrollDown => {
                // Scroll focused pane down (towards live)
                if self.scroll_mode && self.scroll_offset > 0 {
                    self.scroll_offset = self.scroll_offset.saturating_sub(3);
                    if self.scroll_offset == 0 {
                        self.scroll_mode = false;
                    }
                    self.compositor.invalidate();
                    self.needs_redraw = true;
                }
            }
            _ => {}
        }

        Ok(())
    }

    /// Find which pane contains a screen position, return pane ID and buffer coordinates
    fn pane_at_position(&self, x: u16, y: u16) -> Option<(PaneId, u16, u16)> {
        let visible_ids = self.panes.visible_in_view(self.current_view);
        for &pane_id in &visible_ids {
            if let Some(pane) = self.panes.get(pane_id) {
                // Check if position is within pane's content area (excluding header)
                let content_y_start = pane.rect.y + 1; // Skip header row
                let content_y_end = pane.rect.y + pane.rect.height;
                if x >= pane.rect.x && x < pane.rect.x + pane.rect.width
                    && y >= content_y_start && y < content_y_end
                {
                    let buf_x = x - pane.rect.x;
                    let buf_y = y - content_y_start;
                    return Some((pane_id, buf_x, buf_y));
                }
            }
        }
        None
    }

    /// Extract selected text from a pane's buffer
    fn extract_selection(&self, sel: &MouseSelection) -> Option<String> {
        let buffer = self.buffers.get(&sel.pane_id)?;

        // Normalize selection (ensure start <= end)
        let (start_y, start_x, end_y, end_x) = if sel.buf_start_y < sel.buf_end_y
            || (sel.buf_start_y == sel.buf_end_y && sel.buf_start_x <= sel.buf_end_x)
        {
            (sel.buf_start_y, sel.buf_start_x, sel.buf_end_y, sel.buf_end_x)
        } else {
            (sel.buf_end_y, sel.buf_end_x, sel.buf_start_y, sel.buf_start_x)
        };

        let mut result = String::new();
        for y in start_y..=end_y {
            let line_start = if y == start_y { start_x } else { 0 };
            let line_end = if y == end_y { end_x } else { buffer.width().saturating_sub(1) };

            for x in line_start..=line_end {
                let cell = buffer.get(x, y);
                result.push(cell.ch);
            }
            // Trim trailing spaces from each line
            if y < end_y {
                while result.ends_with(' ') {
                    result.pop();
                }
                result.push('\n');
            }
        }
        // Trim trailing spaces from last line
        while result.ends_with(' ') {
            result.pop();
        }

        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    /// Copy text to clipboard via xclip (fallback: OSC 52)
    fn copy_to_clipboard(&mut self, text: &str) -> Result<()> {
        use std::process::{Command, Stdio};

        // Try xclip first (works reliably on X11)
        if let Ok(mut child) = Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
            return Ok(());
        }

        // Try xsel as fallback
        if let Ok(mut child) = Command::new("xsel")
            .args(["--clipboard", "--input"])
            .stdin(Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
            return Ok(());
        }

        // Last resort: OSC 52 (requires terminal support)
        let encoded = base64_encode(text);
        let osc52 = format!("\x1b]52;c;{}\x07", encoded);
        let mut stdout = io::stdout();
        write!(stdout, "{}", osc52)?;
        stdout.flush()?;

        Ok(())
    }

    /// Render the screen
    fn render(&mut self) -> Result<()> {
        if !self.needs_redraw {
            return Ok(());
        }

        let mut stdout = io::stdout();

        // Hide cursor during rendering to avoid ghost cursor
        queue!(stdout, Hide)?;

        // Get visible panes and focused pane
        let visible_ids = self.panes.visible_in_view(self.current_view);
        let focused_id = self.panes.focused().map(|p| p.id);

        // Render only visible panes with window numbers
        for (win_num, &pane_id) in visible_ids.iter().enumerate() {
            if let Some(pane) = self.panes.get(pane_id) {
                // Skip panes with zero-size rect (hidden in monocle mode)
                if pane.rect.width == 0 || pane.rect.height == 0 {
                    continue;
                }
                if let Some(buffer) = self.buffers.get(&pane.id) {
                    // In broadcast mode, all panes are "active"
                    let is_focused = self.broadcast_mode || Some(pane.id) == focused_id;
                    // Content rect starts at y+1 to leave room for header
                    let content_rect = Rect::new(
                        pane.rect.x,
                        pane.rect.y + 1,
                        pane.rect.width,
                        pane.rect.height.saturating_sub(1),
                    );
                    // Only apply scroll offset to focused pane in scroll mode
                    let offset = if self.scroll_mode && is_focused { self.scroll_offset } else { 0 };
                    // Get selection bounds for this pane
                    let selection = self.mouse_selection.as_ref()
                        .filter(|s| s.pane_id == pane.id)
                        .map(|s| (s.buf_start_x, s.buf_start_y, s.buf_end_x, s.buf_end_y));
                    self.compositor.render_pane(&mut stdout, buffer, content_rect, is_focused, offset, selection)?;
                    // Draw window header with number and title
                    self.draw_window_header(&mut stdout, pane.rect, win_num + 1, buffer.title(), is_focused)?;
                }
            }
        }

        // Draw separators between visible panes
        let content_height = self.height.saturating_sub(1);
        if visible_ids.len() > 1 && self.layout.current_name() == "[]=" {
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

        // Position cursor in focused pane (if visible and has size)
        // Don't show cursor in scroll mode since we're viewing history
        if !self.scroll_mode {
            if let Some(pane) = self.panes.focused() {
                if visible_ids.contains(&pane.id) && pane.rect.width > 0 && pane.rect.height > 0 {
                    if let Some(buffer) = self.buffers.get(&pane.id) {
                        let (cx, cy) = buffer.cursor();
                        // Clamp cursor to content area bounds
                        let content_height = pane.rect.height.saturating_sub(1);
                        let clamped_cx = cx.min(pane.rect.width.saturating_sub(1));
                        let clamped_cy = cy.min(content_height.saturating_sub(1));
                        // +1 to account for header row
                        queue!(stdout, MoveTo(pane.rect.x + clamped_cx, pane.rect.y + 1 + clamped_cy))?;
                        // Only show cursor if the application wants it visible
                        if buffer.cursor_visible() {
                            queue!(stdout, Show)?;
                        }
                    }
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

        // Initial padding
        write!(stdout, " ")?;

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

            // Style based on state - muted green theme
            if is_viewed && is_focused_tag {
                // Viewed and focused pane has this tag - brighter green bold
                queue!(stdout, SetForegroundColor(Color::Rgb { r: 120, g: 190, b: 120 }), SetAttribute(Attribute::Bold))?;
            } else if is_viewed {
                // Currently viewing this tag - muted green
                queue!(stdout, SetForegroundColor(Color::Rgb { r: 80, g: 150, b: 80 }))?;
            } else {
                // Has panes but not viewing - dim green
                queue!(stdout, SetForegroundColor(Color::Rgb { r: 60, g: 100, b: 60 }))?;
            }

            write!(stdout, "{}", tag + 1)?;
            queue!(stdout, ResetColor, SetAttribute(Attribute::Reset))?;
            write!(stdout, "  ")?;
        }

        // Show layout name
        queue!(stdout, SetForegroundColor(Color::DarkGrey))?;
        write!(stdout, "{}", self.layout.current_name())?;

        // Show zoom indicator
        if self.zoomed_pane.is_some() {
            queue!(stdout, SetForegroundColor(Color::Magenta), SetAttribute(Attribute::Bold))?;
            write!(stdout, " [Z]")?;
        }

        // Show scroll mode indicator
        if self.scroll_mode {
            queue!(stdout, SetForegroundColor(Color::Yellow), SetAttribute(Attribute::Bold))?;
            if let Some(pane) = self.panes.focused() {
                if let Some(buffer) = self.buffers.get(&pane.id) {
                    let total = buffer.scrollback_len();
                    write!(stdout, " [SCROLL {}/{}]", self.scroll_offset, total)?;
                }
            } else {
                write!(stdout, " [SCROLL]")?;
            }
        }

        queue!(stdout, ResetColor, SetAttribute(Attribute::Reset))?;

        Ok(())
    }

    /// Draw window header with number, title, and line
    fn draw_window_header(&self, stdout: &mut impl Write, rect: Rect, num: usize, title: Option<&str>, is_focused: bool) -> Result<()> {
        use crossterm::style::{SetForegroundColor, Color, Attribute, SetAttribute};

        queue!(stdout, MoveTo(rect.x, rect.y))?;

        if is_focused {
            queue!(stdout, SetForegroundColor(Color::Rgb { r: 120, g: 190, b: 120 }), SetAttribute(Attribute::Bold))?;
        } else {
            queue!(stdout, SetForegroundColor(Color::DarkGrey))?;
        }

        // Draw dashes and window number
        let num_str = format!("──[{}]", num);
        write!(stdout, "{}", num_str)?;

        // Calculate remaining width for title and line
        let mut remaining = rect.width.saturating_sub(num_str.chars().count() as u16) as usize;

        // Draw title if present
        if let Some(title) = title {
            if remaining > 2 {
                write!(stdout, " ")?;
                remaining -= 1;
                // Truncate title if too long (leave room for trailing line)
                let max_title_len = remaining.saturating_sub(1);
                if title.len() <= max_title_len {
                    write!(stdout, "{}", title)?;
                    remaining -= title.len();
                } else if max_title_len > 3 {
                    write!(stdout, "{}…", &title[..max_title_len - 1])?;
                    remaining -= max_title_len;
                }
                write!(stdout, " ")?;
                remaining = remaining.saturating_sub(1);
            }
        }

        // Draw line for the rest of the header
        for _ in 0..remaining {
            write!(stdout, "─")?;
        }

        queue!(stdout, ResetColor, SetAttribute(Attribute::Reset))?;

        Ok(())
    }
}

fn run() -> Result<()> {
    use std::time::Instant;

    let (width, height) = terminal::size().context("Failed to get terminal size")?;

    let mut app = App::new(width, height);

    // Create initial pane
    app.create_pane()?;

    // Set up terminal
    terminal::enable_raw_mode().context("Failed to enable raw mode")?;
    execute!(io::stdout(), EnterAlternateScreen, SetTitle("truetm"))?;

    // Enable Kitty keyboard protocol for unambiguous key handling (Alt+O etc.)
    // This is supported by Kitty, Foot, WezTerm, Alacritty, and others
    let _ = execute!(
        io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );

    // Enable mouse capture for pane-isolated selection
    execute!(io::stdout(), EnableMouseCapture)?;

    // Frame timing - target ~60fps max, but render immediately if idle
    let frame_duration = Duration::from_micros(16667); // ~60fps
    let mut last_render = Instant::now();

    // Main loop
    while app.running {
        // Process all available PTY output first
        let had_pty_data = app.process_pty_messages();

        // Check for input with adaptive timeout:
        // - If we have pending renders and frame time elapsed, render now (0ms timeout)
        // - If PTY data is flowing, use short timeout to stay responsive
        // - Otherwise use longer timeout to reduce CPU usage
        let now = Instant::now();
        let since_render = now.duration_since(last_render);

        let poll_timeout = if app.needs_redraw && since_render >= frame_duration {
            Duration::ZERO // Render immediately
        } else if had_pty_data {
            Duration::from_micros(500) // Quick check for more data
        } else {
            Duration::from_millis(16) // Idle - save CPU
        };

        if event::poll(poll_timeout)? {
            match event::read()? {
                Event::Key(key) => app.handle_key(key)?,
                Event::Mouse(mouse) => app.handle_mouse(mouse)?,
                Event::Resize(w, h) => app.resize(w, h)?,
                _ => {}
            }
        }

        // Render if needed and frame time has elapsed (or if we're idle)
        if app.needs_redraw {
            let now = Instant::now();
            if now.duration_since(last_render) >= frame_duration || !had_pty_data {
                app.render()?;
                last_render = now;
            }
        }

        // Exit if no panes left
        if app.panes.is_empty() {
            app.running = false;
        }
    }

    Ok(())
}

/// Convert a crossterm KeyEvent to bytes to send to PTY
fn key_event_to_bytes(key: &KeyEvent) -> Vec<u8> {
    // Calculate xterm modifier code: 1 + (shift?1:0) + (alt?2:0) + (ctrl?4:0)
    // Result: 2=Shift, 3=Alt, 4=Shift+Alt, 5=Ctrl, 6=Shift+Ctrl, 7=Alt+Ctrl, 8=all
    let modifier = {
        let mut m = 1u8;
        if key.modifiers.contains(KeyModifiers::SHIFT) { m += 1; }
        if key.modifiers.contains(KeyModifiers::ALT) { m += 2; }
        if key.modifiers.contains(KeyModifiers::CONTROL) { m += 4; }
        m
    };
    let has_modifier = modifier > 1;

    match key.code {
        KeyCode::Char(c) => {
            let mut bytes = Vec::new();
            // Alt adds ESC prefix
            if key.modifiers.contains(KeyModifiers::ALT) {
                bytes.push(0x1b);
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                // Ctrl+letter produces control codes 1-26
                if c.is_ascii_alphabetic() {
                    bytes.push((c.to_ascii_lowercase() as u8) - b'a' + 1);
                } else {
                    match c {
                        '[' | '3' => bytes.push(0x1b), // Ctrl+[ = ESC
                        '\\' | '4' => bytes.push(0x1c),
                        ']' | '5' => bytes.push(0x1d),
                        '^' | '6' => bytes.push(0x1e),
                        '_' | '7' => bytes.push(0x1f),
                        '?' | '8' => bytes.push(0x7f), // Ctrl+? = DEL
                        '@' | '2' => bytes.push(0x00), // Ctrl+@ = NUL
                        _ => bytes.extend(c.to_string().as_bytes()),
                    }
                }
            } else {
                bytes.extend(c.to_string().as_bytes());
            }
            bytes
        }
        KeyCode::Enter => {
            // Modifiers on Enter: some apps expect CSI sequences
            if has_modifier {
                format!("\x1b[13;{}u", modifier).into_bytes()
            } else {
                vec![b'\r']
            }
        }
        KeyCode::Tab => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                b"\x1b[9;5u".to_vec() // Ctrl+Tab
            } else if key.modifiers.contains(KeyModifiers::ALT) {
                b"\x1b[9;3u".to_vec() // Alt+Tab (if it reaches us)
            } else {
                vec![b'\t']
            }
        }
        KeyCode::BackTab => b"\x1b[Z".to_vec(), // Shift+Tab
        KeyCode::Backspace => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                vec![0x08] // Ctrl+Backspace: often BS (0x08) or delete word
            } else if key.modifiers.contains(KeyModifiers::ALT) {
                b"\x1b\x7f".to_vec() // Alt+Backspace
            } else {
                vec![0x7f]
            }
        }
        KeyCode::Esc => {
            if key.modifiers.contains(KeyModifiers::ALT) {
                b"\x1b\x1b".to_vec() // Alt+Esc
            } else {
                vec![0x1b]
            }
        }

        // Navigation keys with modifier support (xterm format: CSI 1;modifier X)
        KeyCode::Up => csi_with_modifier(b'A', modifier, has_modifier),
        KeyCode::Down => csi_with_modifier(b'B', modifier, has_modifier),
        KeyCode::Right => csi_with_modifier(b'C', modifier, has_modifier),
        KeyCode::Left => csi_with_modifier(b'D', modifier, has_modifier),
        KeyCode::Home => csi_with_modifier(b'H', modifier, has_modifier),
        KeyCode::End => csi_with_modifier(b'F', modifier, has_modifier),

        // Keys with ~ suffix use different format: CSI number;modifier ~
        KeyCode::Insert => csi_tilde_with_modifier(2, modifier, has_modifier),
        KeyCode::Delete => csi_tilde_with_modifier(3, modifier, has_modifier),
        KeyCode::PageUp => csi_tilde_with_modifier(5, modifier, has_modifier),
        KeyCode::PageDown => csi_tilde_with_modifier(6, modifier, has_modifier),

        // Function keys
        KeyCode::F(n) => f_key_sequence(n, modifier, has_modifier),

        _ => Vec::new(),
    }
}

/// Generate CSI sequence with optional modifier: CSI [1;modifier] final
fn csi_with_modifier(final_byte: u8, modifier: u8, has_modifier: bool) -> Vec<u8> {
    if has_modifier {
        format!("\x1b[1;{}{}", modifier, final_byte as char).into_bytes()
    } else {
        vec![0x1b, b'[', final_byte]
    }
}

/// Generate CSI number ~ sequence with optional modifier: CSI number[;modifier] ~
fn csi_tilde_with_modifier(number: u8, modifier: u8, has_modifier: bool) -> Vec<u8> {
    if has_modifier {
        format!("\x1b[{};{}~", number, modifier).into_bytes()
    } else {
        format!("\x1b[{}~", number).into_bytes()
    }
}

/// Generate function key sequence with optional modifier
fn f_key_sequence(n: u8, modifier: u8, has_modifier: bool) -> Vec<u8> {
    // F1-F4 use SS3 format (no modifier support in standard), F5+ use CSI format
    match n {
        1..=4 if !has_modifier => {
            vec![0x1b, b'O', b'P' + n - 1] // ESC O P/Q/R/S
        }
        1..=4 => {
            // With modifiers, F1-F4 use CSI format
            let code = match n {
                1 => 11, 2 => 12, 3 => 13, 4 => 14,
                _ => return Vec::new(),
            };
            format!("\x1b[{};{}~", code, modifier).into_bytes()
        }
        5..=12 => {
            let code = match n {
                5 => 15, 6 => 17, 7 => 18, 8 => 19,
                9 => 20, 10 => 21, 11 => 23, 12 => 24,
                _ => return Vec::new(),
            };
            if has_modifier {
                format!("\x1b[{};{}~", code, modifier).into_bytes()
            } else {
                format!("\x1b[{}~", code).into_bytes()
            }
        }
        _ => Vec::new(),
    }
}

/// Simple base64 encoder for OSC 52 clipboard
fn base64_encode(input: &str) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let bytes = input.as_bytes();
    let mut result = String::new();

    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;

        let n = (b0 << 16) | (b1 << 8) | b2;

        result.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        result.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);

        if chunk.len() > 1 {
            result.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }

        if chunk.len() > 2 {
            result.push(ALPHABET[(n & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }

    result
}
