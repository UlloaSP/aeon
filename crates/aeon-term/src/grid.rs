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
    pub const HIDDEN: Self = Self(1 << 6);

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
    /// Whether each row continues on the next because the text wrapped, so
    /// copying joins them back into one line.
    wrapped: Vec<bool>,
    rows: usize,
    cols: usize,
}

impl Grid {
    pub fn new(rows: usize, cols: usize) -> Self {
        let (rows, cols) = (rows.max(1), cols.max(1));
        Self {
            cells: vec![Cell::default(); rows * cols],
            wrapped: vec![false; rows],
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

    pub fn wrapped(&self, row: usize) -> bool {
        self.wrapped[row]
    }

    pub fn set_wrapped(&mut self, row: usize, wrapped: bool) {
        self.wrapped[row] = wrapped;
    }

    /// Row contents with trailing blanks trimmed. For tests and debugging.
    pub fn row_text(&self, row: usize) -> String {
        let text: String = self.row(row).iter().map(|c| c.ch).collect();
        text.trim_end().to_owned()
    }

    /// Scrolls rows `top..bottom` up by `n`, filling the freed rows at the
    /// bottom with `blank`.
    pub fn scroll_up(&mut self, top: usize, bottom: usize, n: usize, blank: Cell) {
        let bottom = bottom.min(self.rows);
        let n = n.min(bottom.saturating_sub(top));
        if n == 0 {
            return;
        }
        self.cells
            .copy_within((top + n) * self.cols..bottom * self.cols, top * self.cols);
        self.wrapped.copy_within(top + n..bottom, top);
        self.clear_rows(bottom - n..bottom, blank);
    }

    /// Scrolls rows `top..bottom` down by `n`, filling the freed rows at the
    /// top with `blank`.
    pub fn scroll_down(&mut self, top: usize, bottom: usize, n: usize, blank: Cell) {
        let bottom = bottom.min(self.rows);
        let n = n.min(bottom.saturating_sub(top));
        if n == 0 {
            return;
        }
        self.cells.copy_within(
            top * self.cols..(bottom - n) * self.cols,
            (top + n) * self.cols,
        );
        self.wrapped.copy_within(top..bottom - n, top + n);
        self.clear_rows(top..top + n, blank);
    }

    pub fn clear_rows(&mut self, rows: Range<usize>, blank: Cell) {
        let end = rows.end.min(self.rows);
        if rows.start < end {
            self.cells[rows.start * self.cols..end * self.cols].fill(blank);
            self.wrapped[rows.start..end].fill(false);
        }
    }

    /// Clears from `col` to the end of `row`, inclusive.
    pub fn clear_row_from(&mut self, row: usize, col: usize, blank: Cell) {
        let start = row * self.cols + col.min(self.cols);
        self.cells[start..(row + 1) * self.cols].fill(blank);
        self.wrapped[row] = false;
    }

    /// Clears from the start of `row` to `col`, inclusive.
    pub fn clear_row_to(&mut self, row: usize, col: usize, blank: Cell) {
        let end = row * self.cols + (col + 1).min(self.cols);
        self.cells[row * self.cols..end].fill(blank);
    }

    /// Inserts `n` blanks at `col`, shifting the rest of the row right.
    pub fn insert_cells(&mut self, row: usize, col: usize, n: usize, blank: Cell) {
        let cols = self.cols;
        let col = col.min(cols);
        let n = n.min(cols - col);
        let start = row * cols;
        self.cells
            .copy_within(start + col..start + cols - n, start + col + n);
        self.cells[start + col..start + col + n].fill(blank);
    }

    /// Deletes `n` cells at `col`, shifting the rest of the row left.
    pub fn delete_cells(&mut self, row: usize, col: usize, n: usize, blank: Cell) {
        let cols = self.cols;
        let col = col.min(cols);
        let n = n.min(cols - col);
        let start = row * cols;
        self.cells
            .copy_within(start + col + n..start + cols, start + col);
        self.cells[start + cols - n..start + cols].fill(blank);
    }

    /// Blanks `n` cells from `col`, without moving anything.
    pub fn erase_cells(&mut self, row: usize, col: usize, n: usize, blank: Cell) {
        let start = row * self.cols + col.min(self.cols);
        let end = (start + n).min((row + 1) * self.cols);
        self.cells[start..end].fill(blank);
    }

    /// Keeps the top-left content that still fits. Reflow comes later.
    pub fn resize(&mut self, rows: usize, cols: usize) {
        let mut next = Self::new(rows, cols);
        let n = self.cols.min(next.cols);
        for r in 0..self.rows.min(next.rows) {
            next.cells[r * next.cols..r * next.cols + n].copy_from_slice(&self.row(r)[..n]);
            next.wrapped[r] = self.wrapped[r] && self.cols == next.cols;
        }
        *self = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(rows: usize, cols: usize) -> Grid {
        let mut g = Grid::new(rows, cols);
        for r in 0..rows {
            for c in 0..cols {
                g.cell_mut(r, c).ch = char::from(b'a' + (r * cols + c) as u8);
            }
        }
        g
    }

    #[test]
    fn cell_stays_small() {
        assert!(std::mem::size_of::<Cell>() <= 16);
    }

    #[test]
    fn region_scrolls_leave_the_rest_alone() {
        let mut g = filled(4, 2);
        g.scroll_up(1, 3, 1, Cell::default());
        assert_eq!(
            (0..4).map(|r| g.row_text(r)).collect::<Vec<_>>(),
            ["ab", "ef", "", "gh"]
        );
        let mut g = filled(4, 2);
        g.scroll_down(1, 3, 1, Cell::default());
        assert_eq!(
            (0..4).map(|r| g.row_text(r)).collect::<Vec<_>>(),
            ["ab", "", "cd", "gh"]
        );
    }

    #[test]
    fn insert_delete_and_erase_cells() {
        let mut g = filled(1, 5);
        g.insert_cells(0, 1, 2, Cell::default());
        assert_eq!(g.row_text(0), "a  bc");
        g.delete_cells(0, 1, 2, Cell::default());
        assert_eq!(g.row_text(0), "abc");
        g.erase_cells(0, 0, 2, Cell::default());
        assert_eq!(g.row_text(0), "  c");
    }
}
