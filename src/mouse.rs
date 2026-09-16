//! Mouse reporting passthrough.
//!
//! When a pane application enables xterm mouse tracking (DECSET 1000/1002/
//! 1003, optionally SGR encoding via 1006), truetm forwards the host
//! terminal's mouse events to it instead of acting on them itself. This
//! module holds the mode state and re-encodes crossterm events into the
//! byte sequences the application expects.

use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

/// Which mouse events the application asked to receive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MouseTracking {
    /// No tracking (default): truetm handles the mouse itself.
    #[default]
    Off,
    /// DECSET 1000: button presses, releases and wheel.
    Normal,
    /// DECSET 1002: additionally motion while a button is held.
    Button,
    /// DECSET 1003: additionally all motion.
    Any,
}

impl MouseTracking {
    /// Whether an event of this kind should be forwarded under this mode.
    pub fn wants(self, kind: MouseEventKind) -> bool {
        match kind {
            MouseEventKind::Down(_)
            | MouseEventKind::Up(_)
            | MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => self != MouseTracking::Off,
            MouseEventKind::Drag(_) => matches!(self, MouseTracking::Button | MouseTracking::Any),
            MouseEventKind::Moved => self == MouseTracking::Any,
        }
    }
}

/// Coordinate limit for the legacy (non-SGR) encoding: the value is sent
/// as a single byte offset by 32 and must stay below 256.
const LEGACY_MAX_COORD: u16 = 222;

/// Encode a mouse event for the application. `x`/`y` are 0-based cells
/// relative to the pane. Returns `None` when the event cannot be
/// represented (legacy encoding with coordinates out of range).
pub fn encode(
    kind: MouseEventKind,
    modifiers: KeyModifiers,
    x: u16,
    y: u16,
    sgr: bool,
) -> Option<Vec<u8>> {
    let (code, release) = button_code(kind);
    let cb = code | modifier_bits(modifiers);

    if sgr {
        let suffix = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{};{};{}{}", cb, x + 1, y + 1, suffix).into_bytes());
    }

    if x > LEGACY_MAX_COORD || y > LEGACY_MAX_COORD {
        return None;
    }
    // Legacy X10/normal encoding: releases lose their button identity.
    let cb = if release { (cb & !0b11) | 3 } else { cb };
    Some(vec![
        0x1b,
        b'[',
        b'M',
        (cb + 32) as u8,
        (x + 33) as u8,
        (y + 33) as u8,
    ])
}

/// Base button code per the xterm protocol, and whether this is a release.
fn button_code(kind: MouseEventKind) -> (u16, bool) {
    const MOTION: u16 = 32;
    match kind {
        MouseEventKind::Down(b) => (button_number(b), false),
        MouseEventKind::Up(b) => (button_number(b), true),
        MouseEventKind::Drag(b) => (button_number(b) + MOTION, false),
        MouseEventKind::Moved => (3 + MOTION, false),
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        MouseEventKind::ScrollLeft => (66, false),
        MouseEventKind::ScrollRight => (67, false),
    }
}

fn button_number(button: MouseButton) -> u16 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

fn modifier_bits(modifiers: KeyModifiers) -> u16 {
    let mut bits = 0;
    if modifiers.contains(KeyModifiers::SHIFT) {
        bits |= 4;
    }
    if modifiers.contains(KeyModifiers::ALT) {
        bits |= 8;
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        bits |= 16;
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sgr(kind: MouseEventKind, x: u16, y: u16) -> String {
        String::from_utf8(encode(kind, KeyModifiers::empty(), x, y, true).unwrap()).unwrap()
    }

    #[test]
    fn sgr_wheel_is_one_based() {
        assert_eq!(sgr(MouseEventKind::ScrollUp, 0, 0), "\x1b[<64;1;1M");
        assert_eq!(sgr(MouseEventKind::ScrollDown, 4, 9), "\x1b[<65;5;10M");
    }

    #[test]
    fn sgr_release_uses_lowercase_m_and_keeps_button() {
        assert_eq!(sgr(MouseEventKind::Down(MouseButton::Right), 2, 3), "\x1b[<2;3;4M");
        assert_eq!(sgr(MouseEventKind::Up(MouseButton::Right), 2, 3), "\x1b[<2;3;4m");
    }

    #[test]
    fn sgr_motion_adds_32() {
        assert_eq!(sgr(MouseEventKind::Drag(MouseButton::Left), 0, 0), "\x1b[<32;1;1M");
        assert_eq!(sgr(MouseEventKind::Moved, 0, 0), "\x1b[<35;1;1M");
    }

    #[test]
    fn modifiers_are_or_ed_into_the_button_code() {
        let bytes = encode(
            MouseEventKind::ScrollUp,
            KeyModifiers::CONTROL | KeyModifiers::ALT,
            0,
            0,
            true,
        )
        .unwrap();
        assert_eq!(bytes, b"\x1b[<88;1;1M");
    }

    #[test]
    fn legacy_encoding_offsets_by_32_and_erases_release_button() {
        let press = encode(MouseEventKind::Down(MouseButton::Left), KeyModifiers::empty(), 0, 0, false);
        assert_eq!(press.unwrap(), vec![0x1b, b'[', b'M', 32, 33, 33]);
        let release = encode(MouseEventKind::Up(MouseButton::Left), KeyModifiers::empty(), 0, 0, false);
        assert_eq!(release.unwrap(), vec![0x1b, b'[', b'M', 35, 33, 33]);
    }

    #[test]
    fn legacy_encoding_refuses_out_of_range_coordinates() {
        assert!(encode(MouseEventKind::ScrollUp, KeyModifiers::empty(), 300, 0, false).is_none());
    }

    #[test]
    fn tracking_modes_filter_event_kinds() {
        use MouseEventKind::*;
        assert!(!MouseTracking::Off.wants(ScrollUp));
        assert!(MouseTracking::Normal.wants(ScrollUp));
        assert!(!MouseTracking::Normal.wants(Drag(MouseButton::Left)));
        assert!(!MouseTracking::Normal.wants(Moved));
        assert!(MouseTracking::Button.wants(Drag(MouseButton::Left)));
        assert!(!MouseTracking::Button.wants(Moved));
        assert!(MouseTracking::Any.wants(Moved));
    }
}
