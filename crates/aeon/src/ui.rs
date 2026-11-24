//! Drawing for everything around the terminal: interface text (rendered by
//! Windows, see `sys`), the colour roles derived from the theme, and the few
//! shapes the chrome needs.

use std::collections::HashMap;
use std::rc::Rc;

use crate::render::{Frame, Theme, blend};
use crate::sys::{Face, Run, TextRenderer};

/// The theme's colours given interface roles, the way T3 Code derives its
/// surfaces from a canvas and an accent.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub canvas: u32,
    pub raised: u32,
    pub fg: u32,
    pub muted: u32,
    pub label: u32,
    pub faint: u32,
    pub border: u32,
    pub hover: u32,
    pub highlight: u32,
    pub accent: u32,
    pub on_accent: u32,
    pub red: u32,
    pub green: u32,
    pub yellow: u32,
    pub blue: u32,
    pub magenta: u32,
    pub cyan: u32,
}

impl Palette {
    pub fn new(theme: &Theme) -> Self {
        let (bg, fg) = (theme.bg, theme.fg);
        let toward = |amount: u8| blend(bg, fg, amount);
        let [_, red, green, yellow, blue, magenta, cyan, ..] = theme.ansi;
        Self {
            canvas: bg,
            raised: theme.bar_bg,
            fg,
            muted: toward(170),
            label: toward(150),
            faint: toward(96),
            border: toward(22),
            hover: toward(12),
            highlight: toward(24),
            accent: theme.cursor,
            on_accent: bg,
            red,
            green,
            yellow,
            blue,
            magenta,
            cyan,
        }
    }
}

/// A face at a size and weight, in device pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Font {
    pub face: Face,
    pub px: u32,
    pub weight: u16,
}

pub const REGULAR: u16 = 400;
pub const SEMIBOLD: u16 = 600;

/// Rendered text and widths are cached: the chrome repeats the same strings
/// on every frame, including every frame of a shell printing.
const CACHE_LIMIT: usize = 2048;

/// A string rendered to fit some width.
struct Placed {
    max: u32,
    run: Rc<Run>,
}

pub struct Ui {
    renderer: Option<TextRenderer>,
    /// Keyed by font, then by text, so a lookup needs no allocation.
    runs: HashMap<Font, HashMap<String, Placed>>,
    widths: HashMap<Font, HashMap<String, u32>>,
    cached: usize,
    line_heights: HashMap<Font, u32>,
    pub scale: f64,
}

impl Ui {
    pub fn new(scale: f64) -> Self {
        let renderer = TextRenderer::new();
        if renderer.is_none() {
            eprintln!("aeon: no interface text: GDI is unavailable");
        }
        Self {
            renderer,
            runs: HashMap::new(),
            widths: HashMap::new(),
            cached: 0,
            line_heights: HashMap::new(),
            scale,
        }
    }

    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale;
    }

    /// Logical pixels to device pixels.
    pub fn px(&self, logical: f64) -> u32 {
        (logical * self.scale).round() as u32
    }

    pub fn font(&self, face: Face, logical_px: f64, weight: u16) -> Font {
        Font {
            face,
            px: self.px(logical_px).max(1),
            weight,
        }
    }

    pub fn measure(&mut self, text: &str, font: Font) -> u32 {
        if let Some(w) = self.widths.get(&font).and_then(|m| m.get(text)) {
            return *w;
        }
        let w = self
            .renderer
            .as_mut()
            .map_or(0, |r| r.measure(text, font.face, font.px, font.weight).0);
        self.count_entry();
        self.widths
            .entry(font)
            .or_default()
            .insert(text.to_owned(), w);
        w
    }

    /// Bounds the caches: past the limit, start over.
    fn count_entry(&mut self) {
        self.cached += 1;
        if self.cached > CACHE_LIMIT {
            self.runs.clear();
            self.widths.clear();
            self.cached = 0;
        }
    }

    pub fn line_height(&mut self, font: Font) -> u32 {
        if let Some(h) = self.line_heights.get(&font) {
            return *h;
        }
        let h = self.renderer.as_mut().map_or(font.px, |r| {
            r.measure("Ag", font.face, font.px, font.weight).1
        });
        self.line_heights.insert(font, h);
        h
    }

    /// `text` rendered and cut to fit `max` pixels.
    fn placed(&mut self, text: &str, font: Font, max: u32) -> Option<Rc<Run>> {
        if let Some(placed) = self.runs.get(&font).and_then(|m| m.get(text))
            && placed.max == max
        {
            return Some(placed.run.clone());
        }
        let shown = self.fitted(text, font, max);
        let renderer = self.renderer.as_mut()?;
        let run = Rc::new(renderer.render(&shown, font.face, font.px, font.weight));
        self.count_entry();
        self.runs.entry(font).or_default().insert(
            text.to_owned(),
            Placed {
                max,
                run: run.clone(),
            },
        );
        Some(run)
    }

    /// `text` cut with an ellipsis to fit `max` pixels.
    fn fitted(&mut self, text: &str, font: Font, max: u32) -> String {
        if self.measure(text, font) <= max {
            return text.to_owned();
        }
        let ellipsis = self.measure("\u{2026}", font);
        let Some(renderer) = self.renderer.as_mut() else {
            return String::new();
        };
        let units = renderer.fit(
            text,
            font.face,
            font.px,
            font.weight,
            max.saturating_sub(ellipsis),
        );
        let kept: Vec<u16> = text.encode_utf16().take(units).collect();
        format!("{}\u{2026}", String::from_utf16_lossy(&kept).trim_end())
    }

    /// Draws `text` with its line box starting at (`x`, `top`), cut to `max`
    /// pixels. Returns the width drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        frame: &mut Frame<'_>,
        x: u32,
        top: u32,
        text: &str,
        font: Font,
        color: u32,
        max: u32,
    ) -> u32 {
        if text.is_empty() || max == 0 {
            return 0;
        }
        let Some(run) = self.placed(text, font, max) else {
            return 0;
        };
        blit(frame, &run, x, top, color);
        run.width
    }

    /// Like `text`, centred vertically on `mid`.
    #[allow(clippy::too_many_arguments)]
    pub fn text_mid(
        &mut self,
        frame: &mut Frame<'_>,
        x: u32,
        mid: u32,
        text: &str,
        font: Font,
        color: u32,
        max: u32,
    ) -> u32 {
        let top = mid.saturating_sub(self.line_height(font) / 2);
        self.text(frame, x, top, text, font, color, max)
    }

    /// Like `text_mid`, ending at `right`. Returns where it starts.
    pub fn text_right(
        &mut self,
        frame: &mut Frame<'_>,
        right: u32,
        mid: u32,
        text: &str,
        font: Font,
        color: u32,
    ) -> u32 {
        let x = right.saturating_sub(self.measure(text, font));
        self.text_mid(frame, x, mid, text, font, color, u32::MAX);
        x
    }

    /// An icon from the system icon font, centred on (`cx`, `cy`).
    pub fn icon(
        &mut self,
        frame: &mut Frame<'_>,
        cx: u32,
        cy: u32,
        glyph: char,
        px: f64,
        color: u32,
    ) {
        let font = self.font(Face::Icons, px, REGULAR);
        let mut buf = [0u8; 4];
        let Some(run) = self.placed(glyph.encode_utf8(&mut buf), font, u32::MAX) else {
            return;
        };
        blit(
            frame,
            &run,
            cx.saturating_sub(run.width / 2),
            cy.saturating_sub(run.height / 2),
            color,
        );
    }
}

/// Icons from Segoe Fluent Icons (Windows 11) and Segoe MDL2 Assets, which
/// share these code points.
pub mod icon {
    pub const ADD: char = '\u{E710}';
    pub const SETTINGS: char = '\u{E713}';
    pub const TERMINAL: char = '\u{E756}';
    pub const DOWNLOAD: char = '\u{E896}';
    pub const REFRESH: char = '\u{E72C}';
    pub const BELL: char = '\u{EA8F}';
    pub const MUTED: char = '\u{E708}';
    pub const CHECK: char = '\u{E73E}';
    pub const CROSS: char = '\u{E711}';
    pub const SEARCH: char = '\u{E721}';
    pub const CHEVRON_LEFT: char = '\u{E76B}';
    pub const CHEVRON_RIGHT: char = '\u{E76C}';
}

/// Blends a run's ClearType coverage into the frame, channel by channel.
fn blit(frame: &mut Frame<'_>, run: &Run, x: u32, top: u32, color: u32) {
    for gy in 0..run.height {
        let py = top + gy;
        if py >= frame.height {
            break;
        }
        for gx in 0..run.width {
            let px = x + gx;
            if px >= frame.width {
                break;
            }
            let cov = run.coverage[(gy * run.width + gx) as usize];
            if cov == 0 {
                continue;
            }
            let i = (py * frame.width + px) as usize;
            let dst = frame.pixels[i];
            let channel = |shift: u32| {
                let a = (cov >> shift) & 0xff;
                let s = (color >> shift) & 0xff;
                let d = (dst >> shift) & 0xff;
                ((s * a + d * (255 - a)) / 255) << shift
            };
            frame.pixels[i] = channel(16) | channel(8) | channel(0);
        }
    }
}

/// A filled rectangle with antialiased corners of radius `r`.
#[allow(clippy::too_many_arguments)]
pub fn round_rect(frame: &mut Frame<'_>, x: u32, y: u32, w: u32, h: u32, r: u32, color: u32) {
    let r = r.min(w / 2).min(h / 2);
    let rf = r as f32;
    for py in y..(y + h).min(frame.height) {
        let dy = py - y;
        // Distance into a corner band, measured from the corner's centre.
        let cy = if dy < r {
            Some(rf - dy as f32 - 0.5)
        } else if dy >= h - r {
            Some(dy as f32 + 0.5 - (h - r) as f32)
        } else {
            None
        };
        for px in x..(x + w).min(frame.width) {
            let dx = px - x;
            let cx = if dx < r {
                Some(rf - dx as f32 - 0.5)
            } else if dx >= w - r {
                Some(dx as f32 + 0.5 - (w - r) as f32)
            } else {
                None
            };
            let i = (py * frame.width + px) as usize;
            match (cx, cy) {
                (Some(cx), Some(cy)) => {
                    let d = (cx * cx + cy * cy).sqrt();
                    let cover = (rf - d + 0.5).clamp(0.0, 1.0);
                    if cover > 0.0 {
                        frame.pixels[i] = blend(frame.pixels[i], color, (cover * 255.0) as u8);
                    }
                }
                _ => frame.pixels[i] = color,
            }
        }
    }
}

/// A rounded rectangle with a one-pixel border.
#[allow(clippy::too_many_arguments)]
pub fn card(frame: &mut Frame<'_>, x: u32, y: u32, w: u32, h: u32, r: u32, fill: u32, border: u32) {
    round_rect(frame, x, y, w, h, r, border);
    round_rect(
        frame,
        x + 1,
        y + 1,
        w.saturating_sub(2),
        h.saturating_sub(2),
        r.saturating_sub(1),
        fill,
    );
}

/// An antialiased dot of diameter `d` centred on (`cx`, `cy`).
pub fn dot(frame: &mut Frame<'_>, cx: u32, cy: u32, d: u32, color: u32) {
    let r = d as f32 / 2.0;
    let (fx, fy) = (cx as f32, cy as f32);
    let x0 = cx.saturating_sub(d / 2 + 1);
    let y0 = cy.saturating_sub(d / 2 + 1);
    for py in y0..(cy + d / 2 + 2).min(frame.height) {
        for px in x0..(cx + d / 2 + 2).min(frame.width) {
            let dx = px as f32 + 0.5 - fx;
            let dy = py as f32 + 0.5 - fy;
            let cover = (r - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            if cover > 0.0 {
                let i = (py * frame.width + px) as usize;
                frame.pixels[i] = blend(frame.pixels[i], color, (cover * 255.0) as u8);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_rect_fills_the_middle_and_softens_corners() {
        let mut pixels = vec![0u32; 20 * 20];
        let mut frame = Frame {
            pixels: &mut pixels,
            width: 20,
            height: 20,
        };
        round_rect(&mut frame, 2, 2, 16, 16, 6, 0x00ff_ffff);
        assert_eq!(pixels[10 * 20 + 10], 0x00ff_ffff);
        assert_eq!(pixels[2 * 20 + 2], 0, "the corner itself stays outside");
        assert_eq!(pixels[0], 0, "nothing outside the rectangle");
    }

    #[test]
    fn interface_text_renders_with_coverage() {
        let mut ui = Ui::new(1.0);
        let font = ui.font(Face::Ui, 14.0, REGULAR);
        assert!(ui.measure("Settings", font) > 20);
        let mut pixels = vec![0u32; 200 * 40];
        let mut frame = Frame {
            pixels: &mut pixels,
            width: 200,
            height: 40,
        };
        let drawn = ui.text(&mut frame, 4, 4, "Settings", font, 0x00ff_ffff, 200);
        assert!(drawn > 0);
        assert!(pixels.iter().any(|p| *p != 0), "glyphs reached the frame");
    }

    #[test]
    fn long_text_is_cut_with_an_ellipsis() {
        let mut ui = Ui::new(1.0);
        let font = ui.font(Face::Ui, 14.0, REGULAR);
        let cut = ui.fitted("a session title much too long for the sidebar", font, 80);
        assert!(cut.ends_with('\u{2026}'));
        assert!(ui.measure(&cut, font) <= 80);
    }
}
