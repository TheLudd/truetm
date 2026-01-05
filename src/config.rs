//! User configuration
//!
//! Edit this file to customize keybindings and settings.

use crossterm::event::{KeyCode, KeyModifiers};

// ============================================================================
// GENERAL SETTINGS
// ============================================================================

/// Initial ratio of master pane width (0.1 to 0.9)
pub const MASTER_RATIO: f32 = 0.55;

/// Amount to adjust master width per keypress
pub const MASTER_ADJUST_STEP: f32 = 0.05;

/// Maximum lines stored in scrollback buffer per pane
pub const SCROLLBACK_LINES: usize = 1000;

// ============================================================================
// PREFIX KEY
// ============================================================================

/// The prefix key that activates command mode
/// Default: Ctrl+B (like tmux uses Ctrl+B, screen uses Ctrl+A)
pub const PREFIX_KEY: KeyCode = KeyCode::Char('b');
pub const PREFIX_MODIFIERS: KeyModifiers = KeyModifiers::CONTROL;

// ============================================================================
// KEYBINDINGS (after prefix)
// ============================================================================

// Window management
pub const KEY_QUIT: KeyCode = KeyCode::Char('q');
pub const KEY_NEW_WINDOW: KeyCode = KeyCode::Char('c');
pub const KEY_CLOSE_WINDOW: KeyCode = KeyCode::Char('x');
pub const KEY_FOCUS_NEXT: KeyCode = KeyCode::Char('j');
pub const KEY_FOCUS_PREV: KeyCode = KeyCode::Char('k');
pub const KEY_SWAP_MASTER: KeyCode = KeyCode::Enter;

// Layout
pub const KEY_MASTER_SHRINK: KeyCode = KeyCode::Char('h');
pub const KEY_MASTER_GROW: KeyCode = KeyCode::Char('l');

// Tags
pub const KEY_VIEW_TAG: KeyCode = KeyCode::Char('v');
pub const KEY_SET_TAG: KeyCode = KeyCode::Char('t');
pub const KEY_TOGGLE_TAG: KeyCode = KeyCode::Char('T');

// Modes
pub const KEY_TOGGLE_BROADCAST: KeyCode = KeyCode::Char('a');
pub const KEY_ENTER_SCROLL: KeyCode = KeyCode::Char('[');

// ============================================================================
// SCROLL MODE KEYBINDINGS
// ============================================================================

pub const SCROLL_EXIT_1: KeyCode = KeyCode::Char('q');
pub const SCROLL_EXIT_2: KeyCode = KeyCode::Esc;
pub const SCROLL_UP_LINE_1: KeyCode = KeyCode::Char('k');
pub const SCROLL_UP_LINE_2: KeyCode = KeyCode::Up;
pub const SCROLL_DOWN_LINE_1: KeyCode = KeyCode::Char('j');
pub const SCROLL_DOWN_LINE_2: KeyCode = KeyCode::Down;
pub const SCROLL_UP_PAGE: KeyCode = KeyCode::PageUp;
pub const SCROLL_DOWN_PAGE: KeyCode = KeyCode::PageDown;
pub const SCROLL_TO_TOP: KeyCode = KeyCode::Char('g');
pub const SCROLL_TO_BOTTOM: KeyCode = KeyCode::Char('G');
