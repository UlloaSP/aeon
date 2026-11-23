use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
}

impl Default for Cell {
    fn default() -> Self {
        Self { ch: ' ' }
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

    pub fn scroll_up(&mut self, n: usize) {
        let n = n.min(self.rows);
        self.cells.copy_within(n * self.cols.., 0);
        self.clear_rows(self.rows - n..self.rows);
    }

    pub fn clear_rows(&mut self, rows: Range<usize>) {
        let end = rows.end.min(self.rows);
        if rows.start < end {
            self.cells[rows.start * self.cols..end * self.cols].fill(Cell::default());
        }
    }

    /// Clears from `col` to the end of `row`, inclusive.
    pub fn clear_row_from(&mut self, row: usize, col: usize) {
        let start = row * self.cols + col.min(self.cols);
        self.cells[start..(row + 1) * self.cols].fill(Cell::default());
    }

    /// Clears from the start of `row` to `col`, inclusive.
    pub fn clear_row_to(&mut self, row: usize, col: usize) {
        let end = row * self.cols + (col + 1).min(self.cols);
        self.cells[row * self.cols..end].fill(Cell::default());
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
