//! CPU rendering into a softbuffer frame. No GPU context: a GPU device costs
//! tens of megabytes of resident memory before the first frame.

use aeon_config::{Colors, CursorStyle};
use aeon_term::{Cell, Color, Flags, Match, Point, Terminal};

use crate::chrome::Rect;
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
        let colors = colors.resolve();
        Self {
            fg: colors.foreground.to_u32(),
            bg: colors.background.to_u32(),
            cursor: colors.cursor.to_u32(),
            bar_bg: colors.bar_background.to_u32(),
            ansi: colors.ansi.map(|c| c.to_u32()),
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

/// What is drawn over the terminal's own cells.
#[derive(Default)]
pub struct Overlay<'a> {
    /// Lines the view is scrolled up from the bottom.
    pub scroll: usize,
    /// Selected cells, both ends included, in order.
    pub selection: Option<(Point, Point)>,
    /// Find results, oldest first, and which one is current.
    pub matches: &'a [Match],
    pub current: Option<usize>,
    /// A link under the mouse with Ctrl held: line and columns.
    pub link: Option<(u64, usize, usize)>,
    /// Line ranges drawn on a tinted background: commands that failed.
    pub tints: &'a [(u64, u64)],
    pub tint: u32,
    /// The keyboard is elsewhere: the cursor is drawn as an outline.
    pub hollow_cursor: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    frame: &mut Frame<'_>,
    term: &Terminal,
    atlas: &mut Atlas,
    theme: &Theme,
    bounds: Rect,
    origin: (u32, u32),
    cursor_style: CursorStyle,
    overlay: &Overlay<'_>,
) {
    fill_rect(frame, bounds.x, bounds.y, bounds.w, bounds.h, theme.bg);
    let (left, top) = origin;

    let grid = term.grid();
    let cursor = term.cursor();
    let show_cursor = overlay.scroll == 0 && term.modes().cursor_visible;
    let (cw, ch) = (atlas.cell_width, atlas.cell_height);
    let line = (ch / 16).max(1);
    let first = term.screen_line().saturating_sub(overlay.scroll as u64);
    let selected_bg = blend(theme.bg, theme.ansi[4], 105);
    let match_bg = blend(theme.bg, theme.ansi[3], 95);
    let blank = Cell::default();

    for row in 0..grid.rows() {
        let y = top + row as u32 * ch;
        let number = first + row as u64;
        let cells = term.line(number).unwrap_or_default();
        if overlay
            .tints
            .iter()
            .any(|(a, b)| (*a..*b).contains(&number))
        {
            fill_rect(
                frame,
                left,
                y,
                (bounds.x + bounds.w).saturating_sub(left),
                ch,
                overlay.tint,
            );
        }
        // Find results on this line; `matches` is ordered by line.
        let from = overlay.matches.partition_point(|m| m.start.line < number);
        let to = overlay.matches.partition_point(|m| m.start.line <= number);
        for col in 0..grid.cols() {
            let cell = cells.get(col).unwrap_or(&blank);
            let x = left + col as u32 * cw;
            let at_cursor =
                show_cursor && overlay.scroll == 0 && cursor.row == row && cursor.col == col;
            // A block cursor inverts its cell; bar and underline are drawn on top.
            let block = at_cursor && cursor_style == CursorStyle::Block && !overlay.hollow_cursor;
            let (mut fg, mut bg) = theme.cell_colors(cell, block);
            let here = Point { line: number, col };
            if let Some(i) = (from..to).find(|&i| {
                let m = overlay.matches[i];
                (m.start.col..m.start.col + m.len).contains(&col)
            }) {
                if overlay.current == Some(i) {
                    (fg, bg) = (theme.bg, theme.cursor);
                } else {
                    bg = match_bg;
                }
            } else if overlay
                .selection
                .is_some_and(|(a, b)| a <= here && here <= b)
            {
                bg = selected_bg;
            }
            if bg != theme.bg {
                fill_rect(frame, x, y, cw, ch, bg);
            }
            let linked = overlay
                .link
                .is_some_and(|(l, a, b)| l == number && (a..=b).contains(&col));
            if linked {
                let underline = (y + atlas.baseline as u32 + line).min(y + ch - line);
                fill_rect(frame, x, underline, cw, line, theme.ansi[12]);
            }
            if cell.ch != ' ' && !cell.flags.contains(Flags::HIDDEN) {
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
            if at_cursor && overlay.hollow_cursor {
                fill_rect(frame, x, y, cw, line, theme.cursor);
                fill_rect(frame, x, y + ch - line, cw, line, theme.cursor);
                fill_rect(frame, x, y, line, ch, theme.cursor);
                fill_rect(frame, x + cw - line, y, line, ch, theme.cursor);
            } else if at_cursor {
                let thick = (cw / 6).max(2);
                match cursor_style {
                    CursorStyle::Block => {}
                    CursorStyle::Bar => fill_rect(frame, x, y, thick, ch, theme.cursor),
                    CursorStyle::Underline => {
                        fill_rect(frame, x, y + ch - thick, cw, thick, theme.cursor)
                    }
                }
            }
        }
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
}
