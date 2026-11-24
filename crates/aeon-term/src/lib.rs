//! Terminal state: bytes from the PTY in, a grid of cells out.
//!
//! This crate is pure and synchronous. It never touches the PTY, the window,
//! or the clock, so every behavior is testable by feeding bytes.

mod grid;

use std::borrow::Cow;
use std::collections::VecDeque;

pub use grid::{Cell, Color, Flags, Grid};

/// Cursor position on the screen, zero-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
}

/// A position in the whole history: `line` counts from the first line the
/// terminal ever kept, so it stays valid while scrollback grows and trims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Point {
    pub line: u64,
    pub col: usize,
}

/// Which mouse events the program asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseMode {
    #[default]
    Off,
    /// Presses and releases (1000).
    Click,
    /// Plus motion while a button is down (1002).
    Drag,
    /// Plus all motion (1003).
    Motion,
}

/// Modes programs switch with `CSI ? n h` and `CSI ? n l`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Modes {
    pub cursor_visible: bool,
    /// Arrow keys send `ESC O A` rather than `ESC [ A` (1).
    pub app_cursor: bool,
    pub autowrap: bool,
    /// Pastes are wrapped in `ESC [200~` and `ESC [201~` (2004).
    pub bracketed_paste: bool,
    pub mouse: MouseMode,
    /// Mouse reports use the SGR encoding (1006).
    pub sgr_mouse: bool,
    /// The alternate screen of full-screen programs, which has no scrollback.
    pub alt_screen: bool,
}

impl Default for Modes {
    fn default() -> Self {
        Self {
            cursor_visible: true,
            app_cursor: false,
            autowrap: true,
            bracketed_paste: false,
            mouse: MouseMode::Off,
            sgr_mouse: false,
            alt_screen: false,
        }
    }
}

/// A search hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub start: Point,
    /// Columns covered, on `start.line`.
    pub len: usize,
}

/// One command, from its prompt to where it finished, as the shell marked
/// it with OSC 133 (`A` prompt, `B` input, `C` output, `D;exit` done).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// The line the prompt started on.
    pub prompt: u64,
    /// Where typing started, after the prompt.
    pub input: Option<Point>,
    /// The line output started on, once the command ran.
    pub output: Option<u64>,
    /// The line the shell was on when the command finished.
    pub end: Option<u64>,
    /// The exit status, when the shell reported one.
    pub exit: Option<i32>,
    /// What was typed, read off the screen when it ran.
    pub command: String,
}

/// Things that happened in the shell, for the window to react to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    CommandStarted {
        command: String,
    },
    CommandFinished {
        command: String,
        exit: Option<i32>,
    },
    Bell,
    /// OSC 9;4 progress: a percentage, or `None` when it ended.
    Progress(Option<u8>),
}

/// Lines kept above the screen when nothing else is configured.
pub const DEFAULT_SCROLLBACK: usize = 5000;

pub struct Terminal {
    parser: vte::Parser,
    state: State,
}

/// A line that scrolled off the top, without its trailing blanks, stored as
/// its text and runs of attributes: about a tenth of the 16 bytes a cell
/// takes on screen, which matters for thousands of lines of history.
struct Line {
    text: Box<str>,
    /// How many characters share each set of attributes, in order.
    runs: Box<[Run]>,
    wrapped: bool,
}

#[derive(Clone, Copy)]
struct Run {
    len: u16,
    fg: Color,
    bg: Color,
    flags: Flags,
}

impl Line {
    fn pack(cells: &[Cell], wrapped: bool) -> Self {
        let mut runs: Vec<Run> = Vec::new();
        for cell in cells {
            match runs.last_mut() {
                Some(run)
                    if run.fg == cell.fg
                        && run.bg == cell.bg
                        && run.flags == cell.flags
                        && run.len < u16::MAX =>
                {
                    run.len += 1
                }
                _ => runs.push(Run {
                    len: 1,
                    fg: cell.fg,
                    bg: cell.bg,
                    flags: cell.flags,
                }),
            }
        }
        Self {
            text: cells.iter().map(|c| c.ch).collect::<String>().into(),
            runs: runs.into(),
            wrapped,
        }
    }

    fn unpack(&self) -> Vec<Cell> {
        let mut chars = self.text.chars();
        let mut cells = Vec::with_capacity(self.text.len());
        for run in self.runs.iter() {
            for _ in 0..run.len {
                cells.push(Cell {
                    ch: chars.next().unwrap_or(' '),
                    fg: run.fg,
                    bg: run.bg,
                    flags: run.flags,
                });
            }
        }
        cells
    }
}

struct State {
    grid: Grid,
    alt: Grid,
    scrollback: VecDeque<Line>,
    scrollback_limit: usize,
    /// Lines trimmed off the front of the scrollback, ever.
    dropped: u64,
    /// Lines added to the scrollback since the last `take_scrolled`.
    scrolled: usize,
    cursor: Cursor,
    /// The last column was written; the next print wraps first.
    pending_wrap: bool,
    saved: Option<(Cursor, Cell)>,
    /// Attributes new text is written with. Its `ch` is unused.
    pen: Cell,
    /// Scrolling region, `top..bottom` rows.
    top: usize,
    bottom: usize,
    modes: Modes,
    /// Set when anything visible changed since the last [`Terminal::take_dirty`].
    dirty: bool,
    /// Working directory the shell last reported through OSC 7 or OSC 9;9.
    cwd: Option<String>,
    cwd_changed: bool,
    blocks: VecDeque<Block>,
    events: Vec<Event>,
}

/// Blocks kept, oldest dropped first; far more than a screen of history.
const BLOCK_LIMIT: usize = 2000;

impl Terminal {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self::with_scrollback(rows, cols, DEFAULT_SCROLLBACK)
    }

    pub fn with_scrollback(rows: usize, cols: usize, scrollback: usize) -> Self {
        let grid = Grid::new(rows, cols);
        Self {
            parser: vte::Parser::new(),
            state: State {
                alt: Grid::new(grid.rows(), grid.cols()),
                top: 0,
                bottom: grid.rows(),
                grid,
                scrollback: VecDeque::new(),
                scrollback_limit: scrollback,
                dropped: 0,
                scrolled: 0,
                cursor: Cursor::default(),
                pending_wrap: false,
                saved: None,
                pen: Cell::default(),
                modes: Modes::default(),
                dirty: true,
                cwd: None,
                cwd_changed: false,
                blocks: VecDeque::new(),
                events: Vec::new(),
            },
        }
    }

    /// Feeds raw PTY output.
    pub fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.state, bytes);
    }

    pub fn resize(&mut self, rows: usize, cols: usize) {
        let s = &mut self.state;
        let rows = rows.max(1);
        // Keep the cursor's line on screen: lines above it that no longer
        // fit go to the scrollback, as if the screen had scrolled.
        if s.cursor.row >= rows {
            let n = s.cursor.row + 1 - rows;
            if !s.modes.alt_screen {
                for row in 0..n {
                    s.keep_line(row);
                }
            }
            let blank = Cell::default();
            let height = s.screen().rows();
            s.screen_mut().scroll_up(0, height, n, blank);
            s.cursor.row -= n;
        }
        s.grid.resize(rows, cols);
        s.alt.resize(rows, cols);
        s.top = 0;
        s.bottom = s.grid.rows();
        s.cursor.row = s.cursor.row.min(s.grid.rows() - 1);
        s.cursor.col = s.cursor.col.min(s.grid.cols() - 1);
        s.pending_wrap = false;
        s.dirty = true;
    }

    /// The screen programs draw on: the alternate one while it is active.
    pub fn grid(&self) -> &Grid {
        self.state.screen()
    }

    pub fn cursor(&self) -> Cursor {
        self.state.cursor
    }

    pub fn modes(&self) -> Modes {
        self.state.modes
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

    /// Commands the shell marked, oldest first, while their prompt is still
    /// in the history.
    pub fn blocks(&self) -> impl Iterator<Item = &Block> {
        let first = self.state.dropped;
        self.state.blocks.iter().filter(move |b| b.prompt >= first)
    }

    /// What happened since the last call.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.state.events)
    }

    /// How many lines went into the scrollback since the last call, so a
    /// view scrolled up can stay on the same text.
    pub fn take_scrolled(&mut self) -> usize {
        std::mem::take(&mut self.state.scrolled)
    }

    /// The oldest line still kept.
    pub fn first_line(&self) -> u64 {
        self.state.dropped
    }

    /// The history line shown on the screen's first row.
    pub fn screen_line(&self) -> u64 {
        self.state.dropped + self.state.scrollback.len() as u64
    }

    /// Lines kept above the screen. None while the alternate screen is up.
    pub fn scrollback_len(&self) -> usize {
        if self.state.modes.alt_screen {
            0
        } else {
            self.state.scrollback.len()
        }
    }

    /// One line of history or screen. Scrollback lines are as long as their
    /// text, and unpacked on request; screen lines are as wide as the screen.
    pub fn line(&self, line: u64) -> Option<Cow<'_, [Cell]>> {
        self.line_wrapped(line).map(|(cells, _)| cells)
    }

    fn line_wrapped(&self, line: u64) -> Option<(Cow<'_, [Cell]>, bool)> {
        let s = &self.state;
        let screen = self.screen_line();
        if line >= screen {
            let row = (line - screen) as usize;
            let grid = s.screen();
            return (row < grid.rows()).then(|| (Cow::Borrowed(grid.row(row)), grid.wrapped(row)));
        }
        if s.modes.alt_screen {
            return None;
        }
        let index = line.checked_sub(s.dropped)? as usize;
        s.scrollback
            .get(index)
            .map(|l| (Cow::Owned(l.unpack()), l.wrapped))
    }

    /// The text from `start` to `end`, inclusive, as the clipboard wants it:
    /// trailing blanks trimmed, wrapped lines joined, CRLF between lines.
    pub fn text(&self, start: Point, end: Point) -> String {
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let mut out = String::new();
        for line in start.line..=end.line {
            let Some((cells, wrapped)) = self.line_wrapped(line) else {
                continue;
            };
            let from = if line == start.line { start.col } else { 0 };
            let to = if line == end.line {
                (end.col + 1).min(cells.len())
            } else {
                cells.len()
            };
            let piece: String = cells
                .get(from.min(cells.len())..to.max(from.min(cells.len())))
                .unwrap_or(&[])
                .iter()
                .map(|c| c.ch)
                .collect();
            let joined = wrapped && line != end.line;
            out.push_str(if joined { &piece } else { piece.trim_end() });
            if line != end.line && !joined {
                out.push_str("\r\n");
            }
        }
        out
    }

    /// Every occurrence of `query`, ignoring case, oldest first.
    pub fn find(&self, query: &str) -> Vec<Match> {
        let needle: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
        if needle.is_empty() {
            return Vec::new();
        }
        let mut found = Vec::new();
        let first = if self.state.modes.alt_screen {
            self.screen_line()
        } else {
            self.first_line()
        };
        let end = self.screen_line() + self.grid().rows() as u64;
        for line in first..end {
            let Some(cells) = self.line(line) else {
                continue;
            };
            let hay: Vec<char> = cells
                .iter()
                .map(|c| c.ch.to_lowercase().next().unwrap_or(c.ch))
                .collect();
            let mut col = 0;
            while col + needle.len() <= hay.len() {
                if hay[col..col + needle.len()] == needle[..] {
                    found.push(Match {
                        start: Point { line, col },
                        len: needle.len(),
                    });
                    col += needle.len();
                } else {
                    col += 1;
                }
            }
        }
        found
    }

    /// The word under `point`, as double-click selects it: letters, digits,
    /// and the punctuation of paths and URLs.
    pub fn word_at(&self, point: Point) -> (Point, Point) {
        let Some(cells) = self.line(point.line) else {
            return (point, point);
        };
        let is_word = |c: char| c.is_alphanumeric() || "_-.:/\\~@%+=?&#".contains(c);
        if point.col >= cells.len() || !is_word(cells[point.col].ch) {
            return (point, point);
        }
        let mut start = point.col;
        while start > 0 && is_word(cells[start - 1].ch) {
            start -= 1;
        }
        let mut end = point.col;
        while end + 1 < cells.len() && is_word(cells[end + 1].ch) {
            end += 1;
        }
        (
            Point {
                line: point.line,
                col: start,
            },
            Point {
                line: point.line,
                col: end,
            },
        )
    }
}

impl State {
    /// The history line the cursor is on.
    fn cursor_line(&self) -> u64 {
        self.dropped + self.scrollback.len() as u64 + self.cursor.row as u64
    }

    /// OSC 133 marks from the shell's prompt, which delimit commands.
    fn mark(&mut self, kind: &[u8], rest: &[&[u8]]) {
        if self.modes.alt_screen {
            return;
        }
        let here = Point {
            line: self.cursor_line(),
            col: self.cursor.col,
        };
        match kind {
            b"A" => {
                if self.blocks.len() >= BLOCK_LIMIT {
                    self.blocks.pop_front();
                }
                self.blocks.push_back(Block {
                    prompt: here.line,
                    input: None,
                    output: None,
                    end: None,
                    exit: None,
                    command: String::new(),
                });
            }
            b"B" => {
                if let Some(block) = self.blocks.back_mut() {
                    block.input = Some(here);
                }
            }
            b"C" => {
                let command = self
                    .blocks
                    .back()
                    .and_then(|b| b.input)
                    .map(|start| self.typed_since(start))
                    .unwrap_or_default();
                if let Some(block) = self.blocks.back_mut() {
                    block.output = Some(here.line);
                    block.command = command.clone();
                }
                self.events.push(Event::CommandStarted { command });
            }
            b"D" => {
                let exit = rest
                    .first()
                    .and_then(|v| std::str::from_utf8(v).ok())
                    .and_then(|v| v.trim().parse().ok());
                if let Some(block) = self.blocks.back_mut()
                    && block.end.is_none()
                    && block.output.is_some()
                {
                    block.end = Some(here.line);
                    block.exit = exit;
                    self.events.push(Event::CommandFinished {
                        command: block.command.clone(),
                        exit,
                    });
                }
            }
            _ => {}
        }
    }

    /// The text typed after the prompt, up to the cursor.
    fn typed_since(&self, start: Point) -> String {
        let screen = self.dropped + self.scrollback.len() as u64;
        let mut text = String::new();
        for line in start.line..=self.cursor_line() {
            let Some(row) = line.checked_sub(screen).map(|r| r as usize) else {
                continue;
            };
            if row >= self.grid.rows() {
                break;
            }
            let from = if line == start.line { start.col } else { 0 };
            let cells = self.grid.row(row);
            let piece: String = cells[from.min(cells.len())..]
                .iter()
                .map(|c| c.ch)
                .collect();
            text.push_str(piece.trim_end());
            if !self.grid.wrapped(row) && line != self.cursor_line() {
                text.push(' ');
            }
        }
        text.trim().to_owned()
    }

    fn screen(&self) -> &Grid {
        if self.modes.alt_screen {
            &self.alt
        } else {
            &self.grid
        }
    }

    fn screen_mut(&mut self) -> &mut Grid {
        if self.modes.alt_screen {
            &mut self.alt
        } else {
            &mut self.grid
        }
    }

    /// What erased cells become: blank, keeping the current background
    /// (xterm's background color erase, which full-screen apps rely on).
    fn blank(&self) -> Cell {
        Cell {
            bg: self.pen.bg,
            ..Cell::default()
        }
    }

    fn rows(&self) -> usize {
        self.screen().rows()
    }

    fn cols(&self) -> usize {
        self.screen().cols()
    }

    /// Copies a row of the main screen into the scrollback.
    fn keep_line(&mut self, row: usize) {
        if self.scrollback_limit == 0 {
            return;
        }
        let cells = self.grid.row(row);
        let len = cells
            .iter()
            .rposition(|c| *c != Cell::default())
            .map_or(0, |i| i + 1);
        self.scrollback
            .push_back(Line::pack(&cells[..len], self.grid.wrapped(row)));
        if self.scrollback.len() > self.scrollback_limit {
            self.scrollback.pop_front();
            self.dropped += 1;
        }
        self.scrolled += 1;
    }

    /// Scrolls the region up; lines leaving the top of the whole main
    /// screen are kept in the scrollback.
    fn scroll_up(&mut self, n: usize) {
        let n = n.min(self.bottom - self.top);
        if self.top == 0 && !self.modes.alt_screen {
            for row in 0..n {
                self.keep_line(row);
            }
        }
        let (top, bottom, blank) = (self.top, self.bottom, self.blank());
        self.screen_mut().scroll_up(top, bottom, n, blank);
    }

    fn scroll_down(&mut self, n: usize) {
        let (top, bottom, blank) = (self.top, self.bottom, self.blank());
        self.screen_mut().scroll_down(top, bottom, n, blank);
    }

    fn line_feed(&mut self) {
        if self.cursor.row + 1 == self.bottom {
            self.scroll_up(1);
        } else if self.cursor.row + 1 < self.rows() {
            self.cursor.row += 1;
        }
    }

    fn reverse_index(&mut self) {
        if self.cursor.row == self.top {
            self.scroll_down(1);
        } else {
            self.cursor.row = self.cursor.row.saturating_sub(1);
        }
    }

    fn save_cursor(&mut self) {
        self.saved = Some((self.cursor, self.pen));
    }

    fn restore_cursor(&mut self) {
        if let Some((cursor, pen)) = self.saved {
            self.cursor = Cursor {
                row: cursor.row.min(self.rows() - 1),
                col: cursor.col.min(self.cols() - 1),
            };
            self.pen = pen;
        }
        self.pending_wrap = false;
    }

    fn set_alt_screen(&mut self, on: bool, save: bool) {
        if on == self.modes.alt_screen {
            return;
        }
        if on {
            if save {
                self.save_cursor();
            }
            let blank = self.blank();
            let rows = self.alt.rows();
            self.alt.clear_rows(0..rows, blank);
            self.modes.alt_screen = true;
        } else {
            self.modes.alt_screen = false;
            if save {
                self.restore_cursor();
            }
        }
        self.top = 0;
        self.bottom = self.rows();
    }

    fn set_private_mode(&mut self, mode: u16, on: bool) {
        match mode {
            1 => self.modes.app_cursor = on,
            7 => self.modes.autowrap = on,
            25 => self.modes.cursor_visible = on,
            47 | 1047 => self.set_alt_screen(on, false),
            1049 => self.set_alt_screen(on, true),
            1000 => self.modes.mouse = if on { MouseMode::Click } else { MouseMode::Off },
            1002 => self.modes.mouse = if on { MouseMode::Drag } else { MouseMode::Off },
            1003 => {
                self.modes.mouse = if on {
                    MouseMode::Motion
                } else {
                    MouseMode::Off
                }
            }
            1006 => self.modes.sgr_mouse = on,
            2004 => self.modes.bracketed_paste = on,
            _ => {}
        }
    }

    fn reset(&mut self) {
        let (rows, cols) = (self.grid.rows(), self.grid.cols());
        self.grid = Grid::new(rows, cols);
        self.alt = Grid::new(rows, cols);
        self.cursor = Cursor::default();
        self.pending_wrap = false;
        self.saved = None;
        self.pen = Cell::default();
        self.top = 0;
        self.bottom = rows;
        self.modes = Modes::default();
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
                8 => pen.flags.set(Flags::HIDDEN, true),
                9 => pen.flags.set(Flags::STRIKE, true),
                21 => pen.flags.set(Flags::UNDERLINE, true),
                22 => {
                    pen.flags.set(Flags::BOLD, false);
                    pen.flags.set(Flags::DIM, false);
                }
                23 => pen.flags.set(Flags::ITALIC, false),
                24 => pen.flags.set(Flags::UNDERLINE, false),
                27 => pen.flags.set(Flags::INVERSE, false),
                28 => pen.flags.set(Flags::HIDDEN, false),
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
        if self.pending_wrap {
            self.pending_wrap = false;
            let row = self.cursor.row;
            self.screen_mut().set_wrapped(row, true);
            self.cursor.col = 0;
            self.line_feed();
        }
        let (row, col) = (self.cursor.row, self.cursor.col);
        let pen = self.pen;
        *self.screen_mut().cell_mut(row, col) = Cell { ch: c, ..pen };
        if col + 1 >= self.cols() {
            self.pending_wrap = self.modes.autowrap;
        } else {
            self.cursor.col += 1;
        }
        self.dirty = true;
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => self.line_feed(),
            b'\r' => self.cursor.col = 0,
            0x08 => self.cursor.col = self.cursor.col.saturating_sub(1),
            b'\t' => {
                let next = (self.cursor.col / 8 + 1) * 8;
                self.cursor.col = next.min(self.cols() - 1);
            }
            0x07 => {
                self.events.push(Event::Bell);
                return;
            }
            _ => return,
        }
        self.pending_wrap = false;
        self.dirty = true;
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        match params {
            [b"133", kind, rest @ ..] => {
                self.mark(kind, rest);
                return;
            }
            [b"9", b"4", state, rest @ ..] => {
                let value = rest
                    .first()
                    .and_then(|v| std::str::from_utf8(v).ok())
                    .and_then(|v| v.parse::<u8>().ok())
                    .map(|v| v.min(100));
                let progress = match *state {
                    b"0" => None,
                    _ => Some(value.unwrap_or(0)),
                };
                self.events.push(Event::Progress(progress));
                return;
            }
            _ => {}
        }
        if let Some(path) = reported_path(params)
            && self.cwd.as_deref() != Some(path.as_str())
        {
            self.cwd = Some(path);
            self.cwd_changed = true;
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        if !intermediates.is_empty() {
            return;
        }
        match byte {
            b'7' => self.save_cursor(),
            b'8' => self.restore_cursor(),
            b'D' => self.line_feed(),
            b'E' => {
                self.cursor.col = 0;
                self.line_feed();
            }
            b'M' => self.reverse_index(),
            b'c' => self.reset(),
            _ => return,
        }
        self.pending_wrap = false;
        self.dirty = true;
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        if intermediates == b"?" {
            if matches!(action, 'h' | 'l') {
                for p in params.iter() {
                    self.set_private_mode(p[0], action == 'h');
                }
                self.dirty = true;
            }
            return;
        }
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
        let rows = self.rows();
        let cols = self.cols();
        let (row, col) = (self.cursor.row, self.cursor.col);
        match action {
            'A' => {
                let limit = if row >= self.top { self.top } else { 0 };
                self.cursor.row = row.saturating_sub(arg(1)).max(limit);
            }
            'B' => {
                let limit = if row < self.bottom { self.bottom } else { rows };
                self.cursor.row = (row + arg(1)).min(limit - 1);
            }
            'C' | 'a' => self.cursor.col = (col + arg(1)).min(cols - 1),
            'D' => self.cursor.col = col.saturating_sub(arg(1)),
            'E' => {
                self.cursor.row = (row + arg(1)).min(rows - 1);
                self.cursor.col = 0;
            }
            'F' => {
                self.cursor.row = row.saturating_sub(arg(1));
                self.cursor.col = 0;
            }
            'G' | '`' => self.cursor.col = (arg(1) - 1).min(cols - 1),
            'd' => self.cursor.row = (arg(1) - 1).min(rows - 1),
            'H' | 'f' => {
                self.cursor.row = (arg(1) - 1).min(rows - 1);
                self.cursor.col = (arg(1) - 1).min(cols - 1);
            }
            'J' => match arg(0) {
                0 => {
                    let grid = self.screen_mut();
                    grid.clear_row_from(row, col, blank);
                    grid.clear_rows(row + 1..rows, blank);
                }
                1 => {
                    let grid = self.screen_mut();
                    grid.clear_rows(0..row, blank);
                    grid.clear_row_to(row, col, blank);
                }
                2 => self.screen_mut().clear_rows(0..rows, blank),
                3 => {
                    self.dropped += self.scrollback.len() as u64;
                    self.scrollback.clear();
                }
                _ => return,
            },
            'K' => match arg(0) {
                0 => self.screen_mut().clear_row_from(row, col, blank),
                1 => self.screen_mut().clear_row_to(row, col, blank),
                2 => self.screen_mut().clear_rows(row..row + 1, blank),
                _ => return,
            },
            'L' | 'M' if (self.top..self.bottom).contains(&row) => {
                let (n, bottom) = (arg(1), self.bottom);
                let grid = self.screen_mut();
                if action == 'L' {
                    grid.scroll_down(row, bottom, n, blank);
                } else {
                    grid.scroll_up(row, bottom, n, blank);
                }
                self.cursor.col = 0;
            }
            'S' => self.scroll_up(arg(1)),
            'T' => self.scroll_down(arg(1)),
            '@' => self.screen_mut().insert_cells(row, col, arg(1), blank),
            'P' => self.screen_mut().delete_cells(row, col, arg(1), blank),
            'X' => self.screen_mut().erase_cells(row, col, arg(1), blank),
            'r' => {
                let top = arg(1) - 1;
                let bottom = arg(rows as u16).min(rows);
                if top + 1 < bottom {
                    self.top = top;
                    self.bottom = bottom;
                    self.cursor = Cursor::default();
                }
            }
            's' => self.save_cursor(),
            'u' => self.restore_cursor(),
            _ => return,
        }
        self.pending_wrap = false;
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

    fn screen(t: &Terminal) -> Vec<String> {
        (0..t.grid().rows()).map(|r| t.grid().row_text(r)).collect()
    }

    fn line_text(t: &Terminal, line: u64) -> String {
        let text: String = t
            .line(line)
            .unwrap_or_default()
            .iter()
            .map(|c| c.ch)
            .collect();
        text.trim_end().to_owned()
    }

    #[test]
    fn lines_scrolled_off_the_top_are_kept() {
        let mut t = term(2, 5, "a\r\nb\r\nc\r\nd");
        assert_eq!(t.scrollback_len(), 2);
        assert_eq!(t.take_scrolled(), 2);
        assert_eq!(t.take_scrolled(), 0);
        assert_eq!(line_text(&t, t.first_line()), "a");
        assert_eq!(line_text(&t, t.first_line() + 1), "b");
        assert_eq!(line_text(&t, t.screen_line()), "c");
    }

    #[test]
    fn scrollback_trims_its_oldest_lines() {
        let mut t = Terminal::with_scrollback(1, 5, 2);
        t.advance(b"1\r\n2\r\n3\r\n4\r\n5");
        assert_eq!(t.scrollback_len(), 2);
        assert_eq!(t.first_line(), 2);
        assert_eq!(line_text(&t, 2), "3");
        assert_eq!(t.line(1), None);
    }

    #[test]
    fn last_column_waits_before_wrapping() {
        let t = term(3, 3, "abc\r\nd");
        assert_eq!(screen(&t), ["abc", "d", ""]);
        let t = term(3, 3, "abcd");
        assert!(t.grid().wrapped(0));
        assert!(!t.grid().wrapped(1));
    }

    #[test]
    fn alternate_screen_has_no_scrollback_and_restores() {
        let mut t = term(2, 5, "keep\r\nme\x1b[?1049h");
        assert!(t.modes().alt_screen);
        assert_eq!(screen(&t), ["", ""]);
        t.advance(b"x\r\ny\r\nz");
        assert_eq!(t.scrollback_len(), 0);
        t.advance(b"\x1b[?1049l");
        assert!(!t.modes().alt_screen);
        assert_eq!(screen(&t), ["keep", "me"]);
        assert_eq!(t.cursor(), Cursor { row: 1, col: 2 });
    }

    #[test]
    fn scroll_region_moves_only_its_rows() {
        let t = term(4, 3, "top\r\n\x1b[2;3r\x1b[3;1Hab\r\ncd\r\nef");
        assert_eq!(screen(&t), ["top", "cd", "ef", ""]);
        assert_eq!(
            t.scrollback_len(),
            0,
            "a region below the top keeps nothing"
        );
    }

    #[test]
    fn insert_and_delete_lines_and_reverse_index() {
        let t = term(3, 3, "a\r\nb\r\nc\x1b[2H\x1b[L");
        assert_eq!(screen(&t), ["a", "", "b"]);
        let t = term(3, 3, "a\r\nb\r\nc\x1b[1H\x1b[M");
        assert_eq!(screen(&t), ["b", "c", ""]);
        let t = term(2, 3, "a\r\nb\x1b[H\x1bM");
        assert_eq!(screen(&t), ["", "a"]);
    }

    #[test]
    fn characters_insert_delete_and_erase() {
        let t = term(1, 6, "abcdef\x1b[2G\x1b[2P");
        assert_eq!(t.grid().row_text(0), "adef");
        let t = term(1, 6, "abcd\x1b[2G\x1b[2@");
        assert_eq!(t.grid().row_text(0), "a  bcd");
        let t = term(1, 6, "abcd\x1b[2G\x1b[2X");
        assert_eq!(t.grid().row_text(0), "a  d");
    }

    #[test]
    fn saved_cursor_comes_back() {
        let t = term(3, 5, "\x1b[2;3H\x1b7\x1b[H\x1b8x");
        assert_eq!(t.grid().cell(1, 2).ch, 'x');
    }

    #[test]
    fn private_modes_switch() {
        let mut t = term(2, 5, "\x1b[?1h\x1b[?25l\x1b[?2004h\x1b[?1002h\x1b[?1006h");
        let m = t.modes();
        assert!(m.app_cursor && !m.cursor_visible && m.bracketed_paste && m.sgr_mouse);
        assert_eq!(m.mouse, MouseMode::Drag);
        t.advance(b"\x1b[?1002l\x1b[?25h");
        assert_eq!(t.modes().mouse, MouseMode::Off);
        assert!(t.modes().cursor_visible);
    }

    #[test]
    fn copying_joins_wrapped_lines_and_trims() {
        let t = term(3, 4, "abcdef\r\nxy  ");
        let start = Point { line: 0, col: 0 };
        let end = Point { line: 2, col: 3 };
        assert_eq!(t.text(start, end), "abcdef\r\nxy");
        assert_eq!(t.text(end, start), "abcdef\r\nxy", "order does not matter");
        assert_eq!(
            t.text(Point { line: 0, col: 1 }, Point { line: 0, col: 2 }),
            "bc"
        );
    }

    #[test]
    fn find_ignores_case_across_history() {
        let t = term(2, 10, "Error one\r\nok\r\nerror two");
        let hits = t.find("ERROR");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].start, Point { line: 0, col: 0 });
        assert_eq!(hits[1].start.line, 2);
        assert!(t.find("").is_empty());
    }

    #[test]
    fn double_click_word_covers_paths() {
        let t = term(1, 30, "see src/app.rs:42 now");
        let (a, b) = t.word_at(Point { line: 0, col: 6 });
        assert_eq!((a.col, b.col), (4, 16));
        let (a, b) = t.word_at(Point { line: 0, col: 3 });
        assert_eq!(a, b, "a blank selects nothing more");
    }

    #[test]
    fn shrinking_keeps_the_cursor_line_visible() {
        let mut t = term(4, 5, "a\r\nb\r\nc\r\nd");
        t.resize(2, 5);
        assert_eq!(screen(&t), ["c", "d"]);
        assert_eq!(t.scrollback_len(), 2);
        assert_eq!(t.cursor().row, 1);
    }

    #[test]
    fn scrollback_keeps_colors_when_packed() {
        let t = term(1, 10, "\x1b[31mred\x1b[0m plain\r\nnext");
        let line = t.line(t.first_line()).unwrap();
        assert_eq!(line[0].fg, Color::Indexed(1));
        assert_eq!(line[2].ch, 'd');
        assert_eq!(line[4].fg, Color::Default);
        assert_eq!(line_text(&t, t.first_line()), "red plain");
    }

    #[test]
    fn prompt_marks_make_blocks_and_events() {
        let mut t = term(5, 20, "\x1b]133;A\x1b\\> \x1b]133;B\x1b\\npm test");
        t.advance(
            b"\r\n\x1b]133;C\x1b\\failed\r\n\x1b]133;D;1\x1b\\\x1b]133;A\x1b\\> \x1b]133;B\x1b\\",
        );
        let blocks: Vec<&Block> = t.blocks().collect();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].command, "npm test");
        assert_eq!(blocks[0].prompt, 0);
        assert_eq!(blocks[0].output, Some(1));
        assert_eq!(blocks[0].end, Some(2));
        assert_eq!(blocks[0].exit, Some(1));
        assert_eq!(blocks[1].prompt, 2);
        assert_eq!(
            t.take_events(),
            [
                Event::CommandStarted {
                    command: "npm test".into()
                },
                Event::CommandFinished {
                    command: "npm test".into(),
                    exit: Some(1)
                },
            ]
        );
        assert!(t.take_events().is_empty());
    }

    #[test]
    fn done_without_a_run_command_is_not_a_finish() {
        let mut t = term(
            3,
            10,
            "\x1b]133;A\x1b\\> \x1b]133;B\x1b\\\x1b]133;D;0\x1b\\",
        );
        assert!(t.take_events().is_empty());
    }

    #[test]
    fn bell_and_progress_are_reported() {
        let mut t = term(1, 10, "\x07\x1b]9;4;1;64\x07\x1b]9;4;0;0\x07");
        assert_eq!(
            t.take_events(),
            [
                Event::Bell,
                Event::Progress(Some(64)),
                Event::Progress(None)
            ]
        );
    }

    #[test]
    fn erase_saved_lines_clears_the_scrollback() {
        let t = term(1, 5, "a\r\nb\r\nc\x1b[3J");
        assert_eq!(t.scrollback_len(), 0);
        assert_eq!(t.first_line(), t.screen_line());
    }
}
