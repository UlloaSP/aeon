use std::ops::Range;

/// A color as the shell asked for it. Resolving it to pixels needs the theme,
/// which is the renderer's job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Default,
    /// 0-15 are the theme's ANSI colors, 16-255 the xterm cube and grays.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// SGR attributes, as bits so a cell stays 16 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags(u8);

impl Flags {
    pub const BOLD: Self = Self(1);
    pub const DIM: Self = Self(1 << 1);
    pub const ITALIC: Self = Self(1 << 2);
    pub const UNDERLINE: Self = Self(1 << 3);
    pub const INVERSE: Self = Self(1 << 4);
    pub const STRIKE: Self = Self(1 << 5);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn set(&mut self, other: Self, on: bool) {
        if on {
            self.0 |= other.0;
        } else {
            self.0 &= !other.0;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: Flags,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Color::Default,
            bg: Color::Default,
            flags: Flags::default(),
        }
    }
}

/// Fixed-size screen of cells, stored row-major in one allocation so a scroll
/// is a `copy_within`, not a reallocation.
#[derive(Debug, Clone)]
pub struct Grid {
    cells: Vec<Cell>,
    rows: usize,
    cols: usize,
}

impl Grid {
    pub fn new(rows: usize, cols: usize) -> Self {
        let (rows, cols) = (rows.max(1), cols.max(1));
        Self {
            cells: vec![Cell::default(); rows * cols],
            rows,
            cols,
        }
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn cell(&self, row: usize, col: usize) -> &Cell {
        &self.cells[row * self.cols + col]
    }

    pub fn cell_mut(&mut self, row: usize, col: usize) -> &mut Cell {
        &mut self.cells[row * self.cols + col]
    }

    pub fn row(&self, row: usize) -> &[Cell] {
        &self.cells[row * self.cols..(row + 1) * self.cols]
    }

    /// Row contents with trailing blanks trimmed. For tests and debugging.
    pub fn row_text(&self, row: usize) -> String {
        let text: String = self.row(row).iter().map(|c| c.ch).collect();
        text.trim_end().to_owned()
    }

    /// Scrolls the whole screen up, filling new rows with `blank`.
    pub fn scroll_up(&mut self, n: usize, blank: Cell) {
        let n = n.min(self.rows);
        self.cells.copy_within(n * self.cols.., 0);
        self.clear_rows(self.rows - n..self.rows, blank);
    }

    pub fn clear_rows(&mut self, rows: Range<usize>, blank: Cell) {
        let end = rows.end.min(self.rows);
        if rows.start < end {
            self.cells[rows.start * self.cols..end * self.cols].fill(blank);
        }
    }

    /// Clears from `col` to the end of `row`, inclusive.
    pub fn clear_row_from(&mut self, row: usize, col: usize, blank: Cell) {
        let start = row * self.cols + col.min(self.cols);
        self.cells[start..(row + 1) * self.cols].fill(blank);
    }

    /// Clears from the start of `row` to `col`, inclusive.
    pub fn clear_row_to(&mut self, row: usize, col: usize, blank: Cell) {
        let end = row * self.cols + (col + 1).min(self.cols);
        self.cells[row * self.cols..end].fill(blank);
    }

    /// Keeps the top-left content that still fits. Reflow comes later.
    pub fn resize(&mut self, rows: usize, cols: usize) {
        let mut next = Self::new(rows, cols);
        let n = self.cols.min(next.cols);
        for r in 0..self.rows.min(next.rows) {
            next.cells[r * next.cols..r * next.cols + n].copy_from_slice(&self.row(r)[..n]);
        }
        *self = next;
    }
}

#[cfg(test)]
mod tests {
    use super::Cell;

    #[test]
    fn cell_stays_small() {
        assert!(std::mem::size_of::<Cell>() <= 16);
    }
}
