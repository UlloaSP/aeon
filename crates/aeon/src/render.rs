//! CPU rendering into a softbuffer frame. No GPU context: a GPU device costs
//! tens of megabytes of resident memory before the first frame.

use std::path::Path;

use aeon_config::Colors;
use aeon_context::Context;
use aeon_term::{Cell, Color, Flags, Terminal};

use crate::font::Atlas;

pub struct Frame<'a> {
    pub pixels: &'a mut [u32],
    pub width: u32,
    pub height: u32,
}

/// The config's colors resolved to pixels once, not per cell.
pub struct Theme {
    pub(crate) fg: u32,
    pub(crate) bg: u32,
    pub(crate) cursor: u32,
    pub(crate) bar_bg: u32,
    pub(crate) ansi: [u32; 16],
}

impl Theme {
    pub fn new(colors: &Colors) -> Self {
        Self {
            fg: colors.foreground.to_u32(),
            bg: colors.background.to_u32(),
            cursor: colors.cursor.to_u32(),
            bar_bg: colors.bar_background.to_u32(),
            ansi: colors.ansi().map(|c| c.to_u32()),
        }
    }

    fn resolve(&self, color: Color, default: u32) -> u32 {
        match color {
            Color::Default => default,
            Color::Indexed(i) => self.indexed(i),
            Color::Rgb(r, g, b) => rgb(r, g, b),
        }
    }

    /// The xterm 256-color palette: 16 theme colors, a 6x6x6 cube, 24 grays.
    fn indexed(&self, i: u8) -> u32 {
        match i {
            0..=15 => self.ansi[usize::from(i)],
            16..=231 => {
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                let i = i - 16;
                rgb(level(i / 36), level(i / 6 % 6), level(i % 6))
            }
            232..=255 => {
                let gray = 8 + (i - 232) * 10;
                rgb(gray, gray, gray)
            }
        }
    }

    /// Final (foreground, background) for a cell, after bold-as-bright,
    /// inverse, dim, and the cursor.
    fn cell_colors(&self, cell: &Cell, is_cursor: bool) -> (u32, u32) {
        let fg = match cell.fg {
            // Bold brightens the 8 basic colors, as most terminals do.
            Color::Indexed(i @ 0..=7) if cell.flags.contains(Flags::BOLD) => {
                self.ansi[usize::from(i) + 8]
            }
            color => self.resolve(color, self.fg),
        };
        let bg = self.resolve(cell.bg, self.bg);
        let (mut fg, bg) = if cell.flags.contains(Flags::INVERSE) {
            (bg, fg)
        } else {
            (fg, bg)
        };
        if cell.flags.contains(Flags::DIM) {
            fg = blend(bg, fg, 140);
        }
        if is_cursor {
            (self.bg, self.cursor)
        } else {
            (fg, bg)
        }
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

pub fn draw(
    frame: &mut Frame<'_>,
    term: &Terminal,
    atlas: &mut Atlas,
    theme: &Theme,
    origin: (u32, u32),
) {
    frame.pixels.fill(theme.bg);
    let (left, top) = origin;

    let grid = term.grid();
    let cursor = term.cursor();
    let (cw, ch) = (atlas.cell_width, atlas.cell_height);
    let line = (ch / 16).max(1);

    for row in 0..grid.rows() {
        let y = top + row as u32 * ch;
        for (col, cell) in grid.row(row).iter().enumerate() {
            let x = left + col as u32 * cw;
            let is_cursor = cursor.row == row && cursor.col == col;
            let (fg, bg) = theme.cell_colors(cell, is_cursor);
            if bg != theme.bg {
                fill_rect(frame, x, y, cw, ch, bg);
            }
            if cell.ch != ' ' {
                draw_glyph(frame, atlas, cell.ch, x as i32, y as i32, fg);
                // No bold font is loaded: bold is the same glyph drawn again
                // one stroke to the right.
                if cell.flags.contains(Flags::BOLD) {
                    draw_glyph(frame, atlas, cell.ch, (x + line) as i32, y as i32, fg);
                }
            }
            if cell.flags.contains(Flags::UNDERLINE) {
                let underline = (y + atlas.baseline as u32 + line).min(y + ch - line);
                fill_rect(frame, x, underline, cw, line, fg);
            }
            if cell.flags.contains(Flags::STRIKE) {
                fill_rect(frame, x, y + ch / 2, cw, line, fg);
            }
        }
    }
}

/// The context bar: one row of text across the bottom of the window.
pub fn draw_bar(
    frame: &mut Frame<'_>,
    atlas: &mut Atlas,
    theme: &Theme,
    context: &Context,
    (left, top): (u32, u32),
    padding: u32,
) {
    fill_rect(
        frame,
        left,
        top,
        frame.width.saturating_sub(left),
        frame.height.saturating_sub(top),
        theme.bar_bg,
    );
    let text_y = top + (frame.height.saturating_sub(top)).saturating_sub(atlas.cell_height) / 2;
    let max_cols = (frame.width.saturating_sub(left + 2 * padding) / atlas.cell_width) as usize;

    let mut col = 0;
    let mut put = |atlas: &mut Atlas, text: &str, color: u32| {
        for ch in text.chars() {
            if col >= max_cols {
                return;
            }
            if ch != ' ' {
                let x = left + padding + col as u32 * atlas.cell_width;
                draw_glyph(frame, atlas, ch, x as i32, text_y as i32, color);
            }
            col += 1;
        }
    };

    let [_, red, green, yellow, blue, magenta, ..] = theme.ansi;
    let muted = theme.ansi[8];
    let separator = " \u{2502} ";

    if let Some(cwd) = &context.cwd {
        put(atlas, &display_path(cwd), blue);
    }
    if let Some(git) = &context.git {
        put(atlas, separator, muted);
        put(atlas, &git.head, magenta);
        if git.changed > 0 {
            put(atlas, &format!(" \u{b1}{}", git.changed), yellow);
        }
        if git.ahead > 0 {
            put(atlas, &format!(" \u{2191}{}", git.ahead), green);
        }
        if git.behind > 0 {
            put(atlas, &format!(" \u{2193}{}", git.behind), red);
        }
    }
    for (i, server) in context.servers.iter().enumerate() {
        put(atlas, if i == 0 { separator } else { "  " }, muted);
        put(atlas, "\u{2022} ", green);
        put(
            atlas,
            &format!("{} :{}", server.name, server.port),
            theme.fg,
        );
    }
}

/// The sidebar's measurements, shared by drawing and by mouse hit-testing so
/// the two never disagree.
pub struct Sidebar {
    pub width: u32,
    pad: u32,
    header: u32,
    row: u32,
}

impl Sidebar {
    pub fn new(atlas: &Atlas, scale: f64) -> Self {
        let px = |logical: f64| (logical * scale).round() as u32;
        let pad = px(10.0);
        Self {
            width: px(220.0),
            pad,
            header: atlas.cell_height + 2 * pad,
            row: 2 * atlas.cell_height + 2 * pad,
        }
    }

    /// The session row under a point, if any.
    pub fn row_at(&self, x: f64, y: f64, sessions: usize) -> Option<usize> {
        if x < 0.0 || x >= f64::from(self.width) || y < f64::from(self.header) {
            return None;
        }
        let i = ((y - f64::from(self.header)) / f64::from(self.row)) as usize;
        (i < sessions).then_some(i)
    }

    /// Whether a point is on the header's "+" (new session).
    pub fn is_new_button(&self, x: f64, y: f64, atlas: &Atlas) -> bool {
        let button = f64::from(self.pad + 2 * atlas.cell_width);
        y >= 0.0
            && y < f64::from(self.header)
            && x >= f64::from(self.width) - button
            && x < f64::from(self.width)
    }
}

/// What the sidebar shows for one session.
pub struct SidebarEntry<'a> {
    pub context: &'a Context,
}

pub fn draw_sidebar(
    frame: &mut Frame<'_>,
    atlas: &mut Atlas,
    theme: &Theme,
    sidebar: &Sidebar,
    entries: &[SidebarEntry<'_>],
    active: usize,
) {
    let [_, _, green, yellow, _, magenta, ..] = theme.ansi;
    let muted = theme.ansi[8];
    let (w, pad, cw, ch) = (
        sidebar.width,
        sidebar.pad,
        atlas.cell_width,
        atlas.cell_height,
    );
    fill_rect(frame, 0, 0, w, frame.height, theme.bar_bg);
    fill_rect(
        frame,
        w.saturating_sub(1),
        0,
        1,
        frame.height,
        blend(theme.bar_bg, theme.fg, 24),
    );

    let cols = (w.saturating_sub(2 * pad) / cw) as usize;
    draw_text(
        frame,
        atlas,
        pad,
        pad,
        "Sessions",
        muted,
        cols.saturating_sub(3),
    );
    draw_glyph(
        frame,
        atlas,
        '+',
        (w - pad - cw) as i32,
        pad as i32,
        theme.fg,
    );

    for (i, entry) in entries.iter().enumerate() {
        let top = sidebar.header + i as u32 * sidebar.row;
        if top >= frame.height {
            break;
        }
        if i == active {
            fill_rect(
                frame,
                0,
                top,
                w - 1,
                sidebar.row,
                blend(theme.bar_bg, theme.fg, 22),
            );
            fill_rect(frame, 0, top, (pad / 3).max(2), sidebar.row, theme.cursor);
        }
        let (line1, line2) = (top + pad, top + pad + ch);
        // Ctrl+1..9 selects the first nine; show the number to make that visible.
        let number = if i < 9 {
            char::from(b'1' + i as u8)
        } else {
            ' '
        };
        draw_glyph(frame, atlas, number, pad as i32, line1 as i32, muted);
        let text_x = pad + 2 * cw;
        let text_cols = cols.saturating_sub(2);
        let title = entry
            .context
            .cwd
            .as_deref()
            .map_or_else(|| "shell".to_owned(), session_title);
        let title_color = if i == active {
            theme.fg
        } else {
            blend(theme.bar_bg, theme.fg, 190)
        };
        draw_text(frame, atlas, text_x, line1, &title, title_color, text_cols);

        let mut x = text_x;
        let mut left = text_cols;
        let mut put = |frame: &mut Frame<'_>, atlas: &mut Atlas, text: &str, color: u32| {
            let n = text.chars().count().min(left);
            draw_text(frame, atlas, x, line2, text, color, n);
            x += n as u32 * cw;
            left -= n;
        };
        if let Some(git) = &entry.context.git {
            put(frame, atlas, &git.head, magenta);
            if git.changed > 0 {
                put(frame, atlas, &format!(" \u{b1}{}", git.changed), yellow);
            }
        }
        if !entry.context.servers.is_empty() {
            put(
                frame,
                atlas,
                &format!(" \u{2022}{}", entry.context.servers.len()),
                green,
            );
        }
    }
}

/// The last folder of `path`, or `~` for the home folder itself.
fn session_title(path: &Path) -> String {
    let display = display_path(path);
    if display == "~" {
        return display;
    }
    path.file_name()
        .map_or(display, |name| name.to_string_lossy().into_owned())
}

/// Monospace text from `x`, cut with an ellipsis past `max_cols`.
pub(crate) fn draw_text(
    frame: &mut Frame<'_>,
    atlas: &mut Atlas,
    x: u32,
    y: u32,
    text: &str,
    color: u32,
    max_cols: usize,
) {
    let count = text.chars().count();
    let cut = count > max_cols && max_cols > 0;
    let shown = text
        .chars()
        .take(if cut { max_cols - 1 } else { max_cols })
        .chain(cut.then_some('\u{2026}'));
    for (i, ch) in shown.enumerate() {
        if ch != ' ' {
            let cx = x + i as u32 * atlas.cell_width;
            draw_glyph(frame, atlas, ch, cx as i32, y as i32, color);
        }
    }
}

/// `C:\Users\me\code` -> `~\code`.
fn display_path(path: &Path) -> String {
    let home = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from);
    match home.as_deref().and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~\\{}", rest.display()),
        None => path.display().to_string(),
    }
}

pub(crate) fn fill_rect(frame: &mut Frame<'_>, x: u32, y: u32, w: u32, h: u32, color: u32) {
    for py in y..(y + h).min(frame.height) {
        let start = (py * frame.width + x.min(frame.width)) as usize;
        let end = (py * frame.width + (x + w).min(frame.width)) as usize;
        frame.pixels[start..end].fill(color);
    }
}

pub(crate) fn draw_glyph(
    frame: &mut Frame<'_>,
    atlas: &mut Atlas,
    ch: char,
    x: i32,
    y: i32,
    fg: u32,
) {
    let baseline = atlas.baseline;
    let glyph = atlas.glyph(ch);
    let m = glyph.metrics;
    let left = x + m.xmin;
    let top = y + baseline - m.height as i32 - m.ymin;

    for gy in 0..m.height {
        let py = top + gy as i32;
        if py < 0 || py >= frame.height as i32 {
            continue;
        }
        for gx in 0..m.width {
            let px = left + gx as i32;
            if px < 0 || px >= frame.width as i32 {
                continue;
            }
            let alpha = glyph.coverage[gy * m.width + gx];
            if alpha == 0 {
                continue;
            }
            let i = (py as u32 * frame.width + px as u32) as usize;
            frame.pixels[i] = blend(frame.pixels[i], fg, alpha);
        }
    }
}

pub(crate) fn blend(dst: u32, src: u32, alpha: u8) -> u32 {
    let a = u32::from(alpha);
    let mix = |shift: u32| {
        let d = (dst >> shift) & 0xff;
        let s = (src >> shift) & 0xff;
        ((s * a + d * (255 - a)) / 255) << shift
    };
    mix(16) | mix(8) | mix(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::new(&Colors::default())
    }

    #[test]
    fn blend_endpoints() {
        assert_eq!(blend(0x000000, 0xffffff, 0), 0x000000);
        assert_eq!(blend(0x000000, 0xffffff, 255), 0xffffff);
        assert_eq!(blend(0x102030, 0x102030, 128), 0x102030);
    }

    #[test]
    fn xterm_256_palette() {
        let t = theme();
        assert_eq!(t.indexed(1), t.ansi[1]);
        assert_eq!(t.indexed(16), 0x000000);
        assert_eq!(t.indexed(196), 0xff0000);
        assert_eq!(t.indexed(231), 0xffffff);
        assert_eq!(t.indexed(232), 0x080808);
        assert_eq!(t.indexed(255), 0xeeeeee);
    }

    #[test]
    fn bold_brightens_basic_colors_only() {
        let t = theme();
        let mut cell = Cell {
            fg: Color::Indexed(1),
            ..Cell::default()
        };
        cell.flags.set(Flags::BOLD, true);
        assert_eq!(t.cell_colors(&cell, false).0, t.ansi[9]);
        cell.fg = Color::Indexed(200);
        assert_eq!(t.cell_colors(&cell, false).0, t.indexed(200));
    }

    #[test]
    fn inverse_swaps_and_cursor_wins() {
        let t = theme();
        let mut cell = Cell::default();
        cell.flags.set(Flags::INVERSE, true);
        assert_eq!(t.cell_colors(&cell, false), (t.bg, t.fg));
        assert_eq!(t.cell_colors(&cell, true), (t.bg, t.cursor));
    }

    #[test]
    fn home_is_shortened() {
        let Some(home) = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from) else {
            return;
        };
        assert_eq!(display_path(&home), "~");
        assert_eq!(display_path(&home.join("code")), "~\\code");
    }
}
