//! Terminal state: bytes from the PTY in, a grid of cells out.
//!
//! This crate is pure and synchronous. It never touches the PTY, the window,
//! or the clock, so every behavior is testable by feeding bytes.

mod grid;

pub use grid::{Cell, Grid};

/// Cursor position, zero-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
}

pub struct Terminal {
    parser: vte::Parser,
    state: State,
}

struct State {
    grid: Grid,
    cursor: Cursor,
    /// Set when anything visible changed since the last [`Terminal::take_dirty`].
    dirty: bool,
}

impl Terminal {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            parser: vte::Parser::new(),
            state: State {
                grid: Grid::new(rows, cols),
                cursor: Cursor::default(),
                dirty: true,
            },
        }
    }

    /// Feeds raw PTY output.
    pub fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.state, bytes);
    }

    pub fn resize(&mut self, rows: usize, cols: usize) {
        let s = &mut self.state;
        s.grid.resize(rows, cols);
        s.cursor.row = s.cursor.row.min(s.grid.rows() - 1);
        s.cursor.col = s.cursor.col.min(s.grid.cols() - 1);
        s.dirty = true;
    }

    pub fn grid(&self) -> &Grid {
        &self.state.grid
    }

    pub fn cursor(&self) -> Cursor {
        self.state.cursor
    }

    /// Returns whether a redraw is needed, and clears the flag.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.state.dirty)
    }
}

impl State {
    fn line_feed(&mut self) {
        if self.cursor.row + 1 >= self.grid.rows() {
            self.grid.scroll_up(1);
        } else {
            self.cursor.row += 1;
        }
    }
}

impl vte::Perform for State {
    fn print(&mut self, c: char) {
        if self.cursor.col >= self.grid.cols() {
            self.cursor.col = 0;
            self.line_feed();
        }
        self.grid.cell_mut(self.cursor.row, self.cursor.col).ch = c;
        self.cursor.col += 1;
        self.dirty = true;
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => self.line_feed(),
            b'\r' => self.cursor.col = 0,
            0x08 => self.cursor.col = self.cursor.col.saturating_sub(1),
            b'\t' => {
                let next = (self.cursor.col / 8 + 1) * 8;
                self.cursor.col = next.min(self.grid.cols() - 1);
            }
            _ => return,
        }
        self.dirty = true;
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        if !intermediates.is_empty() {
            return;
        }
        let mut args = params.iter().map(|p| p.first().copied().unwrap_or(0));
        let mut arg = |default: u16| match args.next() {
            Some(0) | None => usize::from(default),
            Some(n) => usize::from(n),
        };
        let rows = self.grid.rows();
        let cols = self.grid.cols();
        let cur = &mut self.cursor;
        match action {
            'A' => cur.row = cur.row.saturating_sub(arg(1)),
            'B' => cur.row = (cur.row + arg(1)).min(rows - 1),
            'C' => cur.col = (cur.col + arg(1)).min(cols - 1),
            'D' => cur.col = cur.col.saturating_sub(arg(1)),
            'G' => cur.col = (arg(1) - 1).min(cols - 1),
            'H' | 'f' => {
                cur.row = (arg(1) - 1).min(rows - 1);
                cur.col = (arg(1) - 1).min(cols - 1);
            }
            'J' => match arg(0) {
                0 => {
                    self.grid.clear_row_from(cur.row, cur.col);
                    self.grid.clear_rows(cur.row + 1..rows);
                }
                1 => {
                    self.grid.clear_rows(0..cur.row);
                    self.grid.clear_row_to(cur.row, cur.col);
                }
                2 | 3 => self.grid.clear_rows(0..rows),
                _ => return,
            },
            'K' => match arg(0) {
                0 => self.grid.clear_row_from(cur.row, cur.col),
                1 => self.grid.clear_row_to(cur.row, cur.col),
                2 => self.grid.clear_rows(cur.row..cur.row + 1),
                _ => return,
            },
            _ => return,
        }
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(rows: usize, cols: usize, input: &str) -> Terminal {
        let mut t = Terminal::new(rows, cols);
        t.advance(input.as_bytes());
        t
    }

    #[test]
    fn prints_and_moves_cursor() {
        let t = term(2, 10, "hi");
        assert_eq!(t.grid().row_text(0), "hi");
        assert_eq!(t.cursor(), Cursor { row: 0, col: 2 });
    }

    #[test]
    fn crlf_starts_next_line() {
        let t = term(3, 10, "one\r\ntwo");
        assert_eq!(t.grid().row_text(0), "one");
        assert_eq!(t.grid().row_text(1), "two");
    }

    #[test]
    fn wraps_at_last_column() {
        let t = term(2, 3, "abcde");
        assert_eq!(t.grid().row_text(0), "abc");
        assert_eq!(t.grid().row_text(1), "de");
    }

    #[test]
    fn line_feed_on_last_row_scrolls() {
        let t = term(2, 5, "a\r\nb\r\nc");
        assert_eq!(t.grid().row_text(0), "b");
        assert_eq!(t.grid().row_text(1), "c");
    }

    #[test]
    fn cup_is_one_based_and_clamped() {
        let t = term(3, 5, "\x1b[2;3Hx\x1b[99;99Hy");
        assert_eq!(t.grid().row_text(1), "  x");
        assert_eq!(t.grid().cell(2, 4).ch, 'y');
    }

    #[test]
    fn erase_display_clears_everything() {
        let t = term(2, 5, "abc\r\ndef\x1b[2J");
        assert_eq!(t.grid().row_text(0), "");
        assert_eq!(t.grid().row_text(1), "");
    }

    #[test]
    fn erase_line_from_cursor() {
        let t = term(1, 6, "abcdef\x1b[4G\x1b[K");
        assert_eq!(t.grid().row_text(0), "abc");
    }

    #[test]
    fn utf8_split_across_reads() {
        let mut t = Terminal::new(1, 4);
        let bytes = "é".as_bytes();
        t.advance(&bytes[..1]);
        t.advance(&bytes[1..]);
        assert_eq!(t.grid().row_text(0), "é");
    }

    #[test]
    fn dirty_flag_is_taken_once() {
        let mut t = term(1, 4, "x");
        assert!(t.take_dirty());
        assert!(!t.take_dirty());
        t.advance(b"\x1b[999Z");
        assert!(!t.take_dirty(), "unknown sequences do not repaint");
    }
}
