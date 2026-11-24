use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use fontdue::{Font, FontSettings, Metrics};

use crate::sys::{FALLBACK_FACES, Face, Run, TextRenderer};

/// Tried in order when the config names no font.
const SYSTEM_FONTS: &[&str] = &[
    r"C:\Windows\Fonts\CascadiaMono.ttf",
    r"C:\Windows\Fonts\consola.ttf",
    r"C:\Windows\Fonts\lucon.ttf",
];

/// Where glyphs the main font lacks come from, such as the prompt's `❯` under
/// Consolas. Loaded only when the first missing glyph shows up.
///
/// Not Segoe UI Symbol or emoji fonts: fontdue builds every glyph of a font
/// when it loads, and Segoe UI Symbol alone costs about 45 MB that way.
/// Glyphs neither font has (`➜`, `⇒`, Nerd Font icons) are drawn one at a
/// time by Windows from [`FALLBACK_FACES`] instead.
const FALLBACK_FONTS: &[&str] = &[r"C:\Windows\Fonts\CascadiaMono.ttf"];

pub fn load(configured: Option<&Path>) -> Result<Font, String> {
    let candidates: Vec<PathBuf> = match configured {
        Some(path) => vec![path.to_owned()],
        None => SYSTEM_FONTS.iter().map(PathBuf::from).collect(),
    };
    for path in &candidates {
        if let Ok(bytes) = std::fs::read(path) {
            return Font::from_bytes(bytes, FontSettings::default())
                .map_err(|e| format!("{}: {e}", path.display()));
        }
    }
    Err(format!("no usable font among {candidates:?}"))
}

fn load_fallbacks() -> Vec<Font> {
    FALLBACK_FONTS
        .iter()
        .filter_map(|path| std::fs::read(path).ok())
        .filter_map(|bytes| Font::from_bytes(bytes, FontSettings::default()).ok())
        .collect()
}

pub struct Glyph {
    pub metrics: Metrics,
    pub coverage: Vec<u8>,
}

/// One font at one pixel size, with every glyph rasterized at most once.
pub struct Atlas {
    font: Rc<Font>,
    /// `None` until a glyph is missing from `font`.
    fallbacks: Option<Vec<Font>>,
    /// Windows' renderer, for glyphs no loaded font has; made on first use.
    system: Option<TextRenderer>,
    /// What each fallback face draws for a character it lacks, at this
    /// size: a glyph that renders the same is not there.
    missing: HashMap<u8, Run>,
    px: f32,
    /// Rows are this many times the font's line height, text centred in them.
    line_height: f32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub baseline: i32,
    glyphs: HashMap<char, Glyph>,
}

impl Atlas {
    pub fn new(font: Rc<Font>, px: f32, line_height: f32) -> Self {
        Self::with_fallbacks(font, None, None, px, line_height)
    }

    fn with_fallbacks(
        font: Rc<Font>,
        fallbacks: Option<Vec<Font>>,
        system: Option<TextRenderer>,
        px: f32,
        line_height: f32,
    ) -> Self {
        let line = font
            .horizontal_line_metrics(px)
            .expect("monospace fonts have horizontal metrics");
        let cell_width = font.metrics('M', px).advance_width.ceil().max(1.0) as u32;
        let natural = line.new_line_size.ceil().max(1.0);
        let line_height = line_height.clamp(1.0, 2.5);
        let cell_height = (natural * line_height).round().max(1.0);
        Self {
            cell_width,
            cell_height: cell_height as u32,
            baseline: (line.ascent + (cell_height - natural) / 2.0).ceil() as i32,
            line_height,
            font,
            fallbacks,
            system,
            missing: HashMap::new(),
            px,
            glyphs: HashMap::new(),
        }
    }

    pub fn with_px(self, px: f32) -> Self {
        Self::with_fallbacks(self.font, self.fallbacks, self.system, px, self.line_height)
    }

    pub fn glyph(&mut self, ch: char) -> &Glyph {
        if !self.glyphs.contains_key(&ch) {
            let glyph = self.rasterize(ch);
            self.glyphs.insert(ch, glyph);
        }
        &self.glyphs[&ch]
    }

    fn rasterize(&mut self, ch: char) -> Glyph {
        let px = self.px;
        let font = if self.font.lookup_glyph_index(ch) != 0 {
            Some(&*self.font)
        } else {
            self.fallbacks
                .get_or_insert_with(load_fallbacks)
                .iter()
                .find(|f| f.lookup_glyph_index(ch) != 0)
        };
        if let Some(font) = font {
            let (metrics, coverage) = font.rasterize(ch, px);
            return Glyph { metrics, coverage };
        }
        // No font has it: a blank, never a box.
        self.system_glyph(ch).unwrap_or(Glyph {
            metrics: Metrics::default(),
            coverage: Vec::new(),
        })
    }

    /// `ch` as Windows draws it from the first fallback face that has it,
    /// on this atlas's baseline and centred in the cell when narrower.
    fn system_glyph(&mut self, ch: char) -> Option<Glyph> {
        if self.system.is_none() {
            self.system = TextRenderer::new();
        }
        let system = self.system.as_mut()?;
        let px = self.px.round() as u32;
        let mut buf = [0u8; 4];
        let text = ch.encode_utf8(&mut buf);
        let (face, run) = (0..FALLBACK_FACES.len() as u8).find_map(|i| {
            let face = Face::Fallback(i);
            let run = system.render(text, face, px, 400);
            // U+10FFFD is a private character no font draws.
            let missing = self
                .missing
                .entry(i)
                .or_insert_with(|| system.render("\u{10FFFD}", face, px, 400));
            let drawn = run.coverage.iter().any(|&c| c != 0);
            (drawn && run.coverage != missing.coverage).then_some((face, run))
        })?;
        let ascent = system.ascent(face, px, 400) as i32;
        // ClearType coverage per channel; the terminal blends one alpha.
        let coverage = run
            .coverage
            .iter()
            .map(|c| ((c >> 16) & 0xff).max((c >> 8) & 0xff).max(c & 0xff) as u8)
            .collect();
        let metrics = Metrics {
            xmin: (self.cell_width.saturating_sub(run.width) / 2) as i32,
            // draw_glyph puts the bottom `ymin` below the baseline.
            ymin: ascent - run.height as i32,
            width: run.width as usize,
            height: run.height as usize,
            advance_width: run.width as f32,
            ..Metrics::default()
        };
        Some(Glyph { metrics, coverage })
    }
}
