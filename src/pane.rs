//! Pane management - PTY handles, pane state, and pane collection

use crate::tag::TagSet;
use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};

/// Unique identifier for a pane
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(pub u32);

/// Rectangle defining a pane's position and size in the terminal
#[derive(Debug, Clone, Copy, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    pub fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self { x, y, width, height }
    }
}

/// Messages from PTY reader threads
pub enum PtyMessage {
    Data { pane_id: PaneId, data: Vec<u8> },
    Exit { pane_id: PaneId },
}

/// A single pane containing a PTY
pub struct Pane {
    pub id: PaneId,
    pub rect: Rect,
    pub tags: TagSet,
    pty_writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    _reader_thread: JoinHandle<()>,
    pub exited: bool,
}

impl Pane {
    /// Create a new pane with a shell and explicit PTY size
    pub fn new_with_size(
        id: PaneId,
        rect: Rect,
        tags: TagSet,
        shell: &str,
        env_vars: &[(String, String)],
        pty_tx: Sender<PtyMessage>,
        pty_cols: u16,
        pty_rows: u16,
    ) -> Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: pty_rows,
                cols: pty_cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY")?;

        // Build command with environment
        let mut cmd = CommandBuilder::new(shell);
        for (key, value) in env_vars {
            cmd.env(key, value);
        }

        // Spawn shell
        let _child = pair.slave.spawn_command(cmd).context("Failed to spawn shell")?;

        // Get reader/writer
        let mut pty_reader = pair.master.try_clone_reader().context("Failed to clone PTY reader")?;
        let pty_writer = pair.master.take_writer().context("Failed to take PTY writer")?;

        // Drop slave after spawning
        drop(pair.slave);

        // Spawn reader thread
        let pane_id = id;
        let reader_thread = thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match pty_reader.read(&mut buf) {
                    Ok(0) => {
                        let _ = pty_tx.send(PtyMessage::Exit { pane_id });
                        break;
                    }
                    Ok(n) => {
                        if pty_tx
                            .send(PtyMessage::Data {
                                pane_id,
                                data: buf[..n].to_vec(),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => {
                        let _ = pty_tx.send(PtyMessage::Exit { pane_id });
                        break;
                    }
                }
            }
        });

        Ok(Self {
            id,
            rect,
            tags,
            pty_writer,
            master: pair.master,
            _reader_thread: reader_thread,
            exited: false,
        })
    }

    /// Write bytes to the PTY
    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        self.pty_writer.write_all(data)?;
        self.pty_writer.flush()?;
        Ok(())
    }

    /// Resize the PTY
    pub fn resize(&self, width: u16, height: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows: height,
                cols: width,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to resize PTY")?;
        Ok(())
    }

    /// Update the pane's rectangle with explicit PTY size
    pub fn set_rect_with_size(&mut self, rect: Rect, pty_cols: u16, pty_rows: u16) -> Result<()> {
        self.resize(pty_cols, pty_rows)?;
        self.rect = rect;
        Ok(())
    }
}

/// Manages a collection of panes
pub struct PaneManager {
    panes: Vec<Pane>,
    focus: Option<usize>,
    next_id: u32,
}

impl PaneManager {
    pub fn new() -> Self {
        Self {
            panes: Vec::new(),
            focus: None,
            next_id: 0,
        }
    }

    /// Add a new pane as master (at front) and return its ID
    pub fn add(&mut self, pane: Pane) -> PaneId {
        let id = pane.id;
        // Insert at front so new pane becomes master
        self.panes.insert(0, pane);
        // Focus the new pane (at index 0)
        self.focus = Some(0);
        id
    }

    /// Get the next available pane ID
    pub fn next_id(&mut self) -> PaneId {
        let id = PaneId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Get the focused pane
    pub fn focused(&self) -> Option<&Pane> {
        self.focus.and_then(|i| self.panes.get(i))
    }

    /// Get the focused pane mutably
    pub fn focused_mut(&mut self) -> Option<&mut Pane> {
        self.focus.and_then(|i| self.panes.get_mut(i))
    }

    /// Get a pane by ID
    pub fn get(&self, id: PaneId) -> Option<&Pane> {
        self.panes.iter().find(|p| p.id == id)
    }

    /// Get a pane by ID mutably
    pub fn get_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.panes.iter_mut().find(|p| p.id == id)
    }

    /// Get all panes
    pub fn all(&self) -> &[Pane] {
        &self.panes
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.panes.is_empty()
    }

    /// Remove a pane by ID
    pub fn remove(&mut self, id: PaneId) -> Option<Pane> {
        if let Some(pos) = self.panes.iter().position(|p| p.id == id) {
            let pane = self.panes.remove(pos);
            // Adjust focus
            if self.panes.is_empty() {
                self.focus = None;
            } else if let Some(f) = self.focus {
                if f >= self.panes.len() {
                    self.focus = Some(self.panes.len() - 1);
                } else if f > pos {
                    self.focus = Some(f - 1);
                }
            }
            Some(pane)
        } else {
            None
        }
    }

    /// Mark a pane as exited
    pub fn mark_exited(&mut self, id: PaneId) {
        if let Some(pane) = self.get_mut(id) {
            pane.exited = true;
        }
    }

    /// Remove all exited panes
    pub fn remove_exited(&mut self) {
        let exited_ids: Vec<_> = self.panes.iter().filter(|p| p.exited).map(|p| p.id).collect();
        for id in exited_ids {
            self.remove(id);
        }
    }

    /// Get pane IDs visible in the given view (have any of the view's tags)
    pub fn visible_in_view(&self, view: TagSet) -> Vec<PaneId> {
        self.panes
            .iter()
            .filter(|p| p.tags.intersects(view))
            .map(|p| p.id)
            .collect()
    }

    /// Check if any pane has the given tag
    pub fn any_with_tag(&self, tag: u8) -> bool {
        self.panes.iter().any(|p| p.tags.contains(tag))
    }

    /// Focus next pane within a view
    pub fn focus_next_in_view(&mut self, view: TagSet) {
        let visible: Vec<usize> = self.panes
            .iter()
            .enumerate()
            .filter(|(_, p)| p.tags.intersects(view))
            .map(|(i, _)| i)
            .collect();

        if visible.is_empty() {
            return;
        }

        let current_pos = self.focus.and_then(|f| visible.iter().position(|&i| i == f));
        let next_pos = match current_pos {
            Some(pos) => (pos + 1) % visible.len(),
            None => 0,
        };
        self.focus = Some(visible[next_pos]);
    }

    /// Focus previous pane within a view
    pub fn focus_prev_in_view(&mut self, view: TagSet) {
        let visible: Vec<usize> = self.panes
            .iter()
            .enumerate()
            .filter(|(_, p)| p.tags.intersects(view))
            .map(|(i, _)| i)
            .collect();

        if visible.is_empty() {
            return;
        }

        let current_pos = self.focus.and_then(|f| visible.iter().position(|&i| i == f));
        let next_pos = match current_pos {
            Some(pos) => if pos == 0 { visible.len() - 1 } else { pos - 1 },
            None => 0,
        };
        self.focus = Some(visible[next_pos]);
    }

    /// Ensure focus is on a pane visible in the view
    pub fn ensure_focus_in_view(&mut self, view: TagSet) {
        // Check if current focus is visible
        if let Some(f) = self.focus {
            if let Some(pane) = self.panes.get(f) {
                if pane.tags.intersects(view) {
                    return; // Already focused on a visible pane
                }
            }
        }
        // Focus first visible pane
        for (i, pane) in self.panes.iter().enumerate() {
            if pane.tags.intersects(view) {
                self.focus = Some(i);
                return;
            }
        }
        // No visible panes
        self.focus = None;
    }

    /// Swap focused pane with master (first visible pane in view)
    /// - If focused on non-master: swap with master, focus stays on pane (now at master)
    /// - If focused on master: swap with second pane, focus the new master (old second)
    pub fn swap_with_master(&mut self, view: TagSet) {
        let visible: Vec<usize> = self.panes
            .iter()
            .enumerate()
            .filter(|(_, p)| p.tags.intersects(view))
            .map(|(i, _)| i)
            .collect();

        if visible.len() < 2 {
            return; // Need at least 2 visible panes to swap
        }

        let master_idx = visible[0];
        let second_idx = visible[1];

        if let Some(focused_idx) = self.focus {
            if focused_idx == master_idx {
                // Focused on master: swap with second, focus the new master
                self.panes.swap(master_idx, second_idx);
                self.focus = Some(master_idx); // Focus new master (old second)
            } else if visible.contains(&focused_idx) {
                // Focused on non-master: swap with master, focus follows pane
                self.panes.swap(master_idx, focused_idx);
                self.focus = Some(master_idx); // Focus pane at master position
            }
        }
    }

    /// Focus a specific pane by ID
    pub fn focus_by_id(&mut self, id: PaneId) {
        if let Some(pos) = self.panes.iter().position(|p| p.id == id) {
            self.focus = Some(pos);
        }
    }
}

impl Default for PaneManager {
    fn default() -> Self {
        Self::new()
    }
}
