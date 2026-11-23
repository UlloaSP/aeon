//! Terminal state: bytes from the PTY in, a grid of cells out.
//!
//! This crate is pure and synchronous. It never touches the PTY, the window,
//! or the clock, so every behavior is testable by feeding bytes.

mod grid;

pub use grid::{Cell, Color, Flags, Grid};

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
    /// Attributes new text is written with. Its `ch` is unused.
    pen: Cell,
    /// Set when anything visible changed since the last [`Terminal::take_dirty`].
    dirty: bool,
    /// Working directory the shell last reported through OSC 7 or OSC 9;9.
    cwd: Option<String>,
    cwd_changed: bool,
}

impl Terminal {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            parser: vte::Parser::new(),
            state: State {
                grid: Grid::new(rows, cols),
                cursor: Cursor::default(),
                pen: Cell::default(),
                dirty: true,
                cwd: None,
                cwd_changed: false,
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

    /// The shell's working directory, once its prompt has reported one.
    pub fn cwd(&self) -> Option<&str> {
        self.state.cwd.as_deref()
    }

    /// Returns whether the reported working directory changed, and clears the flag.
    pub fn take_cwd_changed(&mut self) -> bool {
        std::mem::take(&mut self.state.cwd_changed)
    }
}

impl State {
    /// What erased cells become: blank, keeping the current background
    /// (xterm's background color erase, which full-screen apps rely on).
    fn blank(&self) -> Cell {
        Cell {
            bg: self.pen.bg,
            ..Cell::default()
        }
    }

    fn line_feed(&mut self) {
        if self.cursor.row + 1 >= self.grid.rows() {
            self.grid.scroll_up(1, self.blank());
        } else {
            self.cursor.row += 1;
        }
    }

    fn sgr(&mut self, params: &vte::Params) {
        let pen = &mut self.pen;
        // `CSI m` with no parameters is a reset.
        if params.is_empty() {
            *pen = Cell::default();
            return;
        }
        let mut params = params.iter();
        while let Some(param) = params.next() {
            match param[0] {
                0 => *pen = Cell::default(),
                1 => pen.flags.set(Flags::BOLD, true),
                2 => pen.flags.set(Flags::DIM, true),
                3 => pen.flags.set(Flags::ITALIC, true),
                // `4:0` is the colon form of "no underline".
                4 => pen.flags.set(Flags::UNDERLINE, param.get(1) != Some(&0)),
                7 => pen.flags.set(Flags::INVERSE, true),
                9 => pen.flags.set(Flags::STRIKE, true),
                21 => pen.flags.set(Flags::UNDERLINE, true),
                22 => {
                    pen.flags.set(Flags::BOLD, false);
                    pen.flags.set(Flags::DIM, false);
                }
                23 => pen.flags.set(Flags::ITALIC, false),
                24 => pen.flags.set(Flags::UNDERLINE, false),
                27 => pen.flags.set(Flags::INVERSE, false),
                29 => pen.flags.set(Flags::STRIKE, false),
                n @ 30..=37 => pen.fg = Color::Indexed((n - 30) as u8),
                38 => pen.fg = extended_color(param, &mut params).unwrap_or(pen.fg),
                39 => pen.fg = Color::Default,
                n @ 40..=47 => pen.bg = Color::Indexed((n - 40) as u8),
                48 => pen.bg = extended_color(param, &mut params).unwrap_or(pen.bg),
                49 => pen.bg = Color::Default,
                n @ 90..=97 => pen.fg = Color::Indexed((n - 90 + 8) as u8),
                n @ 100..=107 => pen.bg = Color::Indexed((n - 100 + 8) as u8),
                _ => {}
            }
        }
    }
}

/// Parses the color after a 38 or 48, in either the colon form
/// (`38:5:n`, `38:2::r:g:b`, `38:2:r:g:b`) or the semicolon form
/// (`38;5;n`, `38;2;r;g;b`) that most programs still send.
fn extended_color<'a>(param: &[u16], rest: &mut impl Iterator<Item = &'a [u16]>) -> Option<Color> {
    let byte = |v: u16| v.min(255) as u8;
    if param.len() > 1 {
        return match param[1] {
            5 => param.get(2).map(|&i| Color::Indexed(byte(i))),
            2 => {
                let rgb = if param.len() >= 6 {
                    &param[3..6]
                } else {
                    param.get(2..5)?
                };
                Some(Color::Rgb(byte(rgb[0]), byte(rgb[1]), byte(rgb[2])))
            }
            _ => None,
        };
    }
    let mut value = || rest.next().map(|p| byte(p[0]));
    match value()? {
        5 => Some(Color::Indexed(value()?)),
        2 => Some(Color::Rgb(value()?, value()?, value()?)),
        _ => None,
    }
}

/// Turns an OSC 7 `file://host/C:/path` URL or an OSC 9;9 path into a Windows path.
fn reported_path(params: &[&[u8]]) -> Option<String> {
    let joined = |parts: &[&[u8]]| String::from_utf8_lossy(&parts.join(&b';')).into_owned();
    match params {
        [b"7", url @ ..] if !url.is_empty() => {
            let url = joined(url);
            let rest = url.strip_prefix("file://")?;
            let path = percent_decode(&rest[rest.find('/')?..]);
            // `/C:/Users` -> `C:\Users`. Anything without a drive is not a Windows path.
            let path = path
                .strip_prefix('/')
                .filter(|p| p.as_bytes().get(1) == Some(&b':'))?;
            Some(path.replace('/', "\\"))
        }
        [b"9", b"9", path @ ..] if !path.is_empty() => {
            let path = joined(path);
            Some(path.trim_matches('"').to_owned()).filter(|p| !p.is_empty())
        }
        _ => None,
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let hex = |i: usize| bytes.get(i).and_then(|&b| (b as char).to_digit(16));
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match (bytes[i], hex(i + 1), hex(i + 2)) {
            (b'%', Some(hi), Some(lo)) => {
                out.push((hi * 16 + lo) as u8);
                i += 3;
            }
            (b, ..) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl vte::Perform for State {
    fn print(&mut self, c: char) {
        if self.cursor.col >= self.grid.cols() {
            self.cursor.col = 0;
            self.line_feed();
        }
        *self.grid.cell_mut(self.cursor.row, self.cursor.col) = Cell { ch: c, ..self.pen };
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

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if let Some(path) = reported_path(params)
            && self.cwd.as_deref() != Some(path.as_str())
        {
            self.cwd = Some(path);
            self.cwd_changed = true;
        }
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
        if action == 'm' {
            self.sgr(params);
            return;
        }
        let blank = self.blank();
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
                    self.grid.clear_row_from(cur.row, cur.col, blank);
                    self.grid.clear_rows(cur.row + 1..rows, blank);
                }
                1 => {
                    self.grid.clear_rows(0..cur.row, blank);
                    self.grid.clear_row_to(cur.row, cur.col, blank);
                }
                2 | 3 => self.grid.clear_rows(0..rows, blank),
                _ => return,
            },
            'K' => match arg(0) {
                0 => self.grid.clear_row_from(cur.row, cur.col, blank),
                1 => self.grid.clear_row_to(cur.row, cur.col, blank),
                2 => self.grid.clear_rows(cur.row..cur.row + 1, blank),
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
    fn sgr_basic_and_bright_colors() {
        let t = term(1, 10, "\x1b[31ma\x1b[1;94mb\x1b[0mc");
        let row = t.grid().row(0);
        assert_eq!(row[0].fg, Color::Indexed(1));
        assert_eq!(row[1].fg, Color::Indexed(12));
        assert!(row[1].flags.contains(Flags::BOLD));
        assert_eq!(
            row[2],
            Cell {
                ch: 'c',
                ..Cell::default()
            }
        );
    }

    #[test]
    fn sgr_without_params_resets() {
        let t = term(1, 4, "\x1b[7;41mx\x1b[my");
        assert!(t.grid().cell(0, 0).flags.contains(Flags::INVERSE));
        assert_eq!(
            t.grid().cell(0, 1),
            &Cell {
                ch: 'y',
                ..Cell::default()
            }
        );
    }

    #[test]
    fn sgr_256_and_truecolor_in_both_forms() {
        let t = term(
            1,
            8,
            "\x1b[38;5;208ma\x1b[48;2;1;2;3mb\x1b[38:2::4:5:6mc\x1b[38:5:99md",
        );
        let row = t.grid().row(0);
        assert_eq!(row[0].fg, Color::Indexed(208));
        assert_eq!(row[1].bg, Color::Rgb(1, 2, 3));
        assert_eq!(row[2].fg, Color::Rgb(4, 5, 6));
        assert_eq!(row[3].fg, Color::Indexed(99));
        assert_eq!(row[3].bg, Color::Rgb(1, 2, 3), "earlier attributes persist");
    }

    #[test]
    fn erase_keeps_current_background() {
        let t = term(1, 4, "\x1b[44m\x1b[2K");
        assert!(t.grid().row(0).iter().all(|c| c.bg == Color::Indexed(4)));
    }

    #[test]
    fn osc_9_9_reports_cwd() {
        let mut t = term(1, 4, "\x1b]9;9;\"C:\\Users\\me\"\x1b\\");
        assert_eq!(t.cwd(), Some(r"C:\Users\me"));
        assert!(t.take_cwd_changed());
        t.advance(b"\x1b]9;9;\"C:\\Users\\me\"\x07");
        assert!(!t.take_cwd_changed(), "same directory is not a change");
    }

    #[test]
    fn osc_7_reports_decoded_cwd() {
        let t = term(1, 4, "\x1b]7;file://box/C:/My%20Code/aeon\x1b\\");
        assert_eq!(t.cwd(), Some(r"C:\My Code\aeon"));
    }

    #[test]
    fn osc_7_without_drive_is_ignored() {
        let t = term(1, 4, "\x1b]7;file://box/home/me\x1b\\");
        assert_eq!(t.cwd(), None);
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
