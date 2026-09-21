//! Reflow - re-breaking lines when a pane changes width.
//!
//! The grid stores rendered cells, so a break between two rows can mean two
//! different things: the text ran past the right edge (a *soft* wrap, which
//! must be recalculated at the new width) or the application printed a
//! newline (a *hard* break, which must be preserved). `ScreenBuffer` records
//! which is which as a flag per row; this module joins rows back into
//! logical lines, re-breaks them at the new width and splits the result
//! between scrollback and screen.

use crate::render::{is_wide, Cell, WIDE_CONT};
use std::collections::VecDeque;

/// One row of cells plus whether its text continued onto the next row.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub cells: Vec<Cell>,
    pub wrapped: bool,
}

impl Line {
    pub fn new(cells: Vec<Cell>, wrapped: bool) -> Self {
        Self { cells, wrapped }
    }
}

/// The grid to reflow: `cells` is `width * height` cells in row-major order,
/// `wrapped` one flag per row.
pub struct Grid<'a> {
    pub cells: &'a [Cell],
    pub wrapped: &'a [bool],
    pub width: u16,
    pub height: u16,
    pub cursor: (u16, u16),
}

/// The reflowed state, sized to the new width and height.
pub struct Reflowed {
    pub scrollback: VecDeque<Line>,
    pub cells: Vec<Cell>,
    pub wrapped: Vec<bool>,
    pub cursor: (u16, u16),
}

/// Re-break `scrollback` and `grid` to fit `width` by `height`.
///
/// Both are reflowed together because a resize moves rows across the
/// boundary between them: widening pulls rows back onto the screen,
/// narrowing pushes them into the scrollback.
pub fn reflow(
    scrollback: &VecDeque<Line>,
    grid: Grid,
    width: u16,
    height: u16,
    scrollback_limit: usize,
) -> Reflowed {
    let (logical, marks) = to_logical(scrollback, &grid);
    let (rows, anchor) = to_rows(&logical, marks, width as usize);
    place(
        rows,
        anchor,
        width as usize,
        height as usize,
        scrollback_limit,
    )
}

/// Copy a grid into new dimensions without re-breaking anything, anchored at
/// the top left. Used for the alternate screen, which the running
/// application repaints itself.
pub fn crop(cells: &[Cell], width: u16, height: u16, new_width: u16, new_height: u16) -> Vec<Cell> {
    let (old_w, new_w) = (width as usize, new_width as usize);
    let mut out = vec![Cell::default(); new_w * (new_height as usize)];
    for y in 0..height.min(new_height) as usize {
        for x in 0..width.min(new_width) as usize {
            out[y * new_w + x] = cells[y * old_w + x];
        }
    }
    // A narrower width can cut a wide-char pair at the right edge, leaving a
    // lead glyph whose continuation cell was dropped.
    if new_width < width && new_width > 0 {
        for y in 0..new_height as usize {
            let last = y * new_w + new_w - 1;
            if is_wide(out[last].ch) {
                out[last].ch = ' ';
            }
        }
    }
    out
}

/// A logical line: the cells of one or more rows joined back together.
type Logical = Vec<Cell>;

/// A place in the logical text: which logical line, and which column of it.
type Mark = (usize, usize);

/// Where the screen sat in the text before the resize, as logical marks.
struct Marks {
    top: Mark,
    cursor: Mark,
    room_below: bool,
}

/// Where the screen sits in the reflowed rows.
struct Anchor {
    /// The row the screen started at.
    top: usize,
    /// The row and column the cursor is on.
    cursor: (usize, usize),
    /// Whether the old screen had unused rows below its content. A screen
    /// that was full stays full, pulling rows back out of the scrollback
    /// when re-breaking frees space; a screen with room to spare keeps that
    /// room, so one an application cleared is not refilled.
    room_below: bool,
}

/// Joins rows into logical lines, following each row's wrapped flag.
struct Joiner {
    lines: Vec<Logical>,
    open: bool,
}

impl Joiner {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            open: false,
        }
    }

    /// Index of the logical line the next row will land in.
    fn index(&self) -> usize {
        if self.open && !self.lines.is_empty() {
            self.lines.len() - 1
        } else {
            self.lines.len()
        }
    }

    /// Column the next row's first cell will occupy in that logical line.
    fn offset(&self) -> usize {
        if self.open {
            self.lines.last().map_or(0, |line| line.len())
        } else {
            0
        }
    }

    fn push(&mut self, cells: Vec<Cell>, wrapped: bool) {
        match self.lines.last_mut() {
            Some(last) if self.open => last.extend(cells),
            _ => self.lines.push(cells),
        }
        self.open = wrapped;
    }
}

fn to_logical(scrollback: &VecDeque<Line>, grid: &Grid) -> (Vec<Logical>, Marks) {
    let mut joiner = Joiner::new();
    for line in scrollback {
        joiner.push(line.cells.clone(), line.wrapped);
    }

    let width = grid.width as usize;
    let rows_in_use = rows_in_use(grid);
    let top = (joiner.index(), joiner.offset());
    let mut cursor = (joiner.index(), joiner.offset() + grid.cursor.0 as usize);
    for y in 0..rows_in_use {
        let wrapped = grid.wrapped.get(y).copied().unwrap_or(false);
        let mut cells = grid.cells[y * width..(y + 1) * width].to_vec();
        if !wrapped {
            // Rows are padded to full width; that padding is not content.
            while cells.last() == Some(&Cell::default()) {
                cells.pop();
            }
        }
        if y == grid.cursor.1 as usize {
            cursor = (joiner.index(), joiner.offset() + grid.cursor.0 as usize);
        }
        joiner.push(cells, wrapped);
    }
    (
        joiner.lines,
        Marks {
            top,
            cursor,
            room_below: rows_in_use < grid.height as usize,
        },
    )
}

/// Rows of the grid that hold content: everything down to the last non-blank
/// row, or the cursor row if it sits lower. Blank rows below that are just
/// unused screen and must not become blank lines in the reflowed text.
fn rows_in_use(grid: &Grid) -> usize {
    let width = grid.width as usize;
    let mut last = grid.cursor.1 as usize;
    for y in (0..grid.height as usize).rev() {
        if grid.cells[y * width..(y + 1) * width]
            .iter()
            .any(|cell| *cell != Cell::default())
        {
            last = last.max(y);
            break;
        }
    }
    (last + 1).min(grid.height as usize)
}

fn to_rows(logical: &[Logical], marks: Marks, width: usize) -> (Vec<Line>, Anchor) {
    let mut rows: Vec<Line> = Vec::new();
    let mut top = 0;
    let mut cursor = (0, 0);
    for (index, line) in logical.iter().enumerate() {
        let first = rows.len();
        let starts = break_line(line, width, &mut rows);
        if index == marks.top.0 {
            top = row_of(&starts, first, marks.top.1, width).min(rows.len());
        }
        if index == marks.cursor.0 {
            cursor = locate(&starts, first, marks.cursor.1, width, &mut rows);
        }
    }
    (
        rows,
        Anchor {
            top,
            cursor,
            room_below: marks.room_below,
        },
    )
}

/// Row a column of a logical line now lives on, without disturbing `rows`.
fn row_of(starts: &[usize], first_row: usize, col: usize, width: usize) -> usize {
    for (k, &start) in starts.iter().enumerate().rev() {
        if col >= start {
            return first_row + k + usize::from(col - start >= width);
        }
    }
    first_row
}

/// Break one logical line into rows of at most `width` cells, never
/// splitting a double-width character from its continuation cell. Returns
/// the starting column of each row produced.
fn break_line(line: &[Cell], width: usize, rows: &mut Vec<Line>) -> Vec<usize> {
    if line.is_empty() {
        rows.push(Line::new(Vec::new(), false));
        return vec![0];
    }

    let mut starts = Vec::new();
    let mut i = 0;
    while i < line.len() {
        let mut end = (i + width).min(line.len());
        if end < line.len() && line[end].ch == WIDE_CONT {
            end -= 1;
        }
        if end <= i {
            end = i + 1; // one-column pane: a wide glyph cannot fit at all
        }
        let mut cells = line[i..end].to_vec();
        if let Some(last) = cells.last_mut() {
            if is_wide(last.ch) {
                last.ch = ' '; // its continuation cell did not fit
            }
        }
        starts.push(i);
        rows.push(Line::new(cells, end < line.len()));
        i = end;
    }
    starts
}

/// Map a column in a logical line to the row it now lives on.
fn locate(
    starts: &[usize],
    first_row: usize,
    col: usize,
    width: usize,
    rows: &mut Vec<Line>,
) -> (usize, usize) {
    for (k, &start) in starts.iter().enumerate().rev() {
        if col < start {
            continue;
        }
        if col - start < width {
            return (first_row + k, col - start);
        }
        // The cursor sits just past a full row, so it belongs at the start of
        // the next one - which does not exist yet if nothing was printed there.
        let next = first_row + k + 1;
        if next == rows.len() {
            rows.push(Line::new(Vec::new(), false));
        }
        return (next, 0);
    }
    (first_row, 0)
}

/// Split the reflowed rows between scrollback and screen.
///
/// The screen stays where it was in the text: it starts at the row it
/// started at before, so re-breaking grows the text into the unused rows
/// below it rather than scrolling it. A screen that was full has no unused
/// rows, so it is instead kept full from the end of the text, which is what
/// pulls rows back out of the scrollback when widening rejoins them. Either
/// way the cursor row has to end up on screen, and that wins over the
/// anchor.
fn place(rows: Vec<Line>, anchor: Anchor, width: usize, height: usize, limit: usize) -> Reflowed {
    let Anchor {
        top,
        cursor,
        room_below,
    } = anchor;
    let start = if room_below {
        top
    } else {
        top.min(rows.len().saturating_sub(height))
    }
    .clamp(cursor.0.saturating_sub(height.saturating_sub(1)), cursor.0);

    let mut cells = vec![Cell::default(); width * height];
    let mut wrapped = vec![false; height];
    for (y, row) in rows[start..].iter().take(height).enumerate() {
        for (x, cell) in row.cells.iter().take(width).enumerate() {
            cells[y * width + x] = *cell;
        }
        wrapped[y] = row.wrapped;
    }

    let mut scrollback: VecDeque<Line> = rows[..start].iter().cloned().collect();
    while scrollback.len() > limit {
        scrollback.pop_front();
    }

    Reflowed {
        scrollback,
        cells,
        wrapped,
        cursor: (
            cursor.1.min(width.saturating_sub(1)) as u16,
            (cursor.0 - start).min(height.saturating_sub(1)) as u16,
        ),
    }
}
