//! End-to-end test: spawns the real truetm binary inside a PTY, types into
//! the shell running in its pane, and asserts on the screen truetm draws.
//!
//! The host side is modeled by a small, independent terminal grid (below) so
//! the compositor's actual output is verified, not just the internal buffer.
//! Inside the pane, output-tests/checks.sh exercises the escape-sequence
//! handling end to end and self-verifies via cursor position reports.

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const COLS: usize = 100;
const ROWS: usize = 30;

// ---------------------------------------------------------------------------
// Minimal host-side terminal model. Deliberately independent from truetm's
// own parser: it only understands what the compositor emits (cursor moves,
// erases, SGR it ignores) plus UTF-8 and wide characters.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum State {
    Normal,
    Esc,
    Csi,
    Osc,
    Charset,
}

struct HostGrid {
    cells: Vec<char>,
    cx: usize,
    cy: usize,
    state: State,
    seq: Vec<u8>,
    utf8: Vec<u8>,
    utf8_need: usize,
}

impl HostGrid {
    fn new() -> Self {
        Self {
            cells: vec![' '; COLS * ROWS],
            cx: 0,
            cy: 0,
            state: State::Normal,
            seq: Vec::new(),
            utf8: Vec::new(),
            utf8_need: 0,
        }
    }

    fn feed(&mut self, data: &[u8]) {
        for &b in data {
            self.byte(b);
        }
    }

    fn byte(&mut self, b: u8) {
        match self.state {
            State::Normal => self.normal(b),
            State::Esc => {
                self.state = match b {
                    b'[' => {
                        self.seq.clear();
                        State::Csi
                    }
                    b']' => {
                        self.seq.clear();
                        State::Osc
                    }
                    b'(' | b')' => State::Charset,
                    _ => State::Normal, // ESC 7/8/=/>/M/c/\ etc - ignore
                };
            }
            State::Charset => self.state = State::Normal,
            State::Csi => {
                if (0x40..=0x7e).contains(&b) {
                    self.seq.push(b);
                    self.execute_csi();
                    self.state = State::Normal;
                } else {
                    self.seq.push(b);
                }
            }
            State::Osc => {
                if b == 0x07 || (b == b'\\' && self.seq.last() == Some(&0x1b)) {
                    self.state = State::Normal;
                    self.seq.clear();
                } else {
                    self.seq.push(b);
                }
            }
        }
    }

    fn normal(&mut self, b: u8) {
        if self.utf8_need > 0 {
            if b & 0xC0 == 0x80 {
                self.utf8.push(b);
                self.utf8_need -= 1;
                if self.utf8_need == 0 {
                    let buf = std::mem::take(&mut self.utf8);
                    if let Ok(s) = std::str::from_utf8(&buf) {
                        for ch in s.chars() {
                            self.put(ch);
                        }
                    }
                }
            } else {
                self.utf8.clear();
                self.utf8_need = 0;
                self.normal(b);
            }
            return;
        }
        match b {
            0x1b => self.state = State::Esc,
            b'\r' => self.cx = 0,
            b'\n' => self.linefeed(),
            0x08 => self.cx = self.cx.saturating_sub(1),
            0x07 => {}
            0xC0..=0xDF => {
                self.utf8 = vec![b];
                self.utf8_need = 1;
            }
            0xE0..=0xEF => {
                self.utf8 = vec![b];
                self.utf8_need = 2;
            }
            0xF0..=0xF7 => {
                self.utf8 = vec![b];
                self.utf8_need = 3;
            }
            0x20..=0x7E => self.put(b as char),
            _ => {}
        }
    }

    fn put(&mut self, ch: char) {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w == 0 {
            return;
        }
        if self.cx + w > COLS {
            self.cx = 0;
            self.linefeed();
        }
        self.cells[self.cy * COLS + self.cx] = ch;
        if w == 2 {
            self.cells[self.cy * COLS + self.cx + 1] = '\0';
        }
        self.cx += w;
    }

    fn linefeed(&mut self) {
        if self.cy + 1 >= ROWS {
            self.cells.copy_within(COLS.., 0);
            for c in &mut self.cells[(ROWS - 1) * COLS..] {
                *c = ' ';
            }
        } else {
            self.cy += 1;
        }
    }

    fn execute_csi(&mut self) {
        let fin = *self.seq.last().unwrap();
        let params_str = String::from_utf8_lossy(&self.seq[..self.seq.len() - 1]).to_string();
        if params_str.starts_with(['?', '>', '<', '=']) {
            return; // private modes (cursor show/hide, mouse, ...) - ignore
        }
        let p: Vec<usize> = params_str.split(';').filter_map(|s| s.parse().ok()).collect();
        let n1 = *p.first().unwrap_or(&1).max(&1);
        match fin {
            b'H' | b'f' => {
                self.cy = (n1 - 1).min(ROWS - 1);
                self.cx = (*p.get(1).unwrap_or(&1).max(&1) - 1).min(COLS - 1);
            }
            b'A' => self.cy = self.cy.saturating_sub(n1),
            b'B' => self.cy = (self.cy + n1).min(ROWS - 1),
            b'C' => self.cx = (self.cx + n1).min(COLS - 1),
            b'D' => self.cx = self.cx.saturating_sub(n1),
            b'G' => self.cx = (n1 - 1).min(COLS - 1),
            b'd' => self.cy = (n1 - 1).min(ROWS - 1),
            b'J' => {
                let mode = *p.first().unwrap_or(&0);
                let cur = self.cy * COLS + self.cx;
                match mode {
                    0 => self.cells[cur..].fill(' '),
                    1 => self.cells[..=cur].fill(' '),
                    _ => self.cells.fill(' '),
                }
            }
            b'K' => {
                let mode = *p.first().unwrap_or(&0);
                let (s, e) = match mode {
                    1 => (self.cy * COLS, self.cy * COLS + self.cx + 1),
                    2 => (self.cy * COLS, (self.cy + 1) * COLS),
                    _ => (self.cy * COLS + self.cx, (self.cy + 1) * COLS),
                };
                self.cells[s..e].fill(' ');
            }
            _ => {} // m, h, l, r, q, s, u, n, c, ... - ignore
        }
    }

    fn text(&self) -> String {
        (0..ROWS)
            .map(|y| {
                self.cells[y * COLS..(y + 1) * COLS]
                    .iter()
                    .filter(|&&c| c != '\0')
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Truetm {
    grid: HostGrid,
    rx: mpsc::Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl Truetm {
    fn spawn() -> Self {
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: ROWS as u16,
                cols: COLS as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");

        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_truetm"));
        cmd.cwd(env!("CARGO_MANIFEST_DIR"));
        cmd.env("TERM", "xterm-256color");
        cmd.env("SHELL", "/bin/bash");
        // Point HOME at a temp dir so the pane shell starts with a plain
        // default prompt instead of the developer's shell configuration.
        cmd.env("HOME", std::env::temp_dir());

        let child = pair.slave.spawn_command(cmd).expect("spawn truetm");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().expect("reader");
        let writer = pair.master.take_writer().expect("writer");
        // Keep the master alive for the duration of the test
        Box::leak(Box::new(pair.master));

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });

        Self {
            grid: HostGrid::new(),
            rx,
            writer,
            child,
        }
    }

    fn type_line(&mut self, s: &str) {
        self.writer.write_all(s.as_bytes()).expect("write");
        self.writer.write_all(b"\r").expect("write");
        self.writer.flush().expect("flush");
    }

    fn pump(&mut self) {
        while let Ok(chunk) = self.rx.try_recv() {
            self.grid.feed(&chunk);
        }
    }

    /// Wait until the screen contains `needle`. Panics with a screen dump on
    /// timeout, or if `abort_on` shows up first.
    fn wait_for(&mut self, needle: &str, abort_on: Option<&str>, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            self.pump();
            let screen = self.grid.text();
            if screen.contains(needle) {
                return;
            }
            if let Some(bad) = abort_on {
                if screen.contains(bad) {
                    panic!("found \"{bad}\" while waiting for \"{needle}\"\nscreen:\n{screen}");
                }
            }
            if Instant::now() > deadline {
                panic!("timed out waiting for \"{needle}\"\nscreen:\n{screen}");
            }
            if let Ok(chunk) = self.rx.recv_timeout(Duration::from_millis(50)) {
                self.grid.feed(&chunk);
            }
        }
    }
}

#[test]
fn truetm_end_to_end() {
    let mut tm = Truetm::spawn();

    // Shell prompt appears inside the pane
    tm.wait_for("$", None, Duration::from_secs(15));

    // Plain output round-trips through buffer + compositor.
    // (Built via %s so the echoed command line can't satisfy the wait.)
    tm.type_line("printf 'e2e-%s\\n' boot-ok");
    tm.wait_for("e2e-boot-ok", None, Duration::from_secs(10));

    // Wide glyph: the following text must land exactly one column after the
    // emoji's two cells - catches compositor cursor drift on wide chars
    tm.type_line("printf '%s after-wide\\n' \"$(printf '\\xf0\\x9f\\x99\\x82')A\"");
    tm.wait_for("🙂A after-wide", None, Duration::from_secs(10));

    // Kitty keyboard query mid-line must not teleport the cursor
    tm.type_line("printf 'kitty-%b-ok\\n' 'AB\\x1b[?uCD'");
    tm.wait_for("kitty-ABCD-ok", None, Duration::from_secs(10));

    // Full escape-sequence suite, self-verified inside the pane via DSR
    tm.type_line("bash output-tests/checks.sh");
    tm.wait_for(
        "ALL CHECKS PASSED",
        Some("FAIL "),
        Duration::from_secs(30),
    );

    // Quit truetm cleanly: prefix (Ctrl+B) then Q
    tm.writer.write_all(&[0x02]).unwrap();
    tm.writer.write_all(b"Q").unwrap();
    tm.writer.flush().unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(Some(_)) = tm.child.try_wait() {
            break;
        }
        if Instant::now() > deadline {
            let _ = tm.child.kill();
            panic!("truetm did not exit after prefix+Q");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
