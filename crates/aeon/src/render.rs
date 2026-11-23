//! CPU rendering into a softbuffer frame. No GPU context: a GPU device costs
//! tens of megabytes of resident memory before the first frame.

use aeon_config::Colors;
use aeon_term::Terminal;

use crate::font::Atlas;

pub struct Frame<'a> {
    pub pixels: &'a mut [u32],
    pub width: u32,
    pub height: u32,
}

pub fn draw(
    frame: &mut Frame<'_>,
    term: &Terminal,
    atlas: &mut Atlas,
    colors: &Colors,
    padding: u32,
) {
    let bg = colors.background.to_u32();
    frame.pixels.fill(bg);

    let grid = term.grid();
    let cursor = term.cursor();
    let (cw, ch) = (atlas.cell_width, atlas.cell_height);

    for row in 0..grid.rows() {
        let y = padding + row as u32 * ch;
        for (col, cell) in grid.row(row).iter().enumerate() {
            let x = padding + col as u32 * cw;
            let is_cursor = cursor.row == row && cursor.col == col;
            let fg = if is_cursor {
                fill_rect(frame, x, y, cw, ch, colors.cursor.to_u32());
                bg
            } else {
                colors.foreground.to_u32()
            };
            if cell.ch != ' ' {
                draw_glyph(frame, atlas, cell.ch, x as i32, y as i32, fg);
            }
        }
    }
}

fn fill_rect(frame: &mut Frame<'_>, x: u32, y: u32, w: u32, h: u32, color: u32) {
    for py in y..(y + h).min(frame.height) {
        let start = (py * frame.width + x.min(frame.width)) as usize;
        let end = (py * frame.width + (x + w).min(frame.width)) as usize;
        frame.pixels[start..end].fill(color);
    }
}

fn draw_glyph(frame: &mut Frame<'_>, atlas: &mut Atlas, ch: char, x: i32, y: i32, fg: u32) {
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

fn blend(dst: u32, src: u32, alpha: u8) -> u32 {
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
    use super::blend;

    #[test]
    fn blend_endpoints() {
        assert_eq!(blend(0x000000, 0xffffff, 0), 0x000000);
        assert_eq!(blend(0x000000, 0xffffff, 255), 0xffffff);
        assert_eq!(blend(0x102030, 0x102030, 128), 0x102030);
    }
}
