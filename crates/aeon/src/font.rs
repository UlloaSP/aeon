use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use fontdue::{Font, FontSettings, Metrics};

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
/// Glyphs neither font has render as a box until fallback rasterizes on demand.
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
    px: f32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub baseline: i32,
    glyphs: HashMap<char, Glyph>,
}

impl Atlas {
    pub fn new(font: Rc<Font>, px: f32) -> Self {
        Self::with_fallbacks(font, None, px)
    }

    fn with_fallbacks(font: Rc<Font>, fallbacks: Option<Vec<Font>>, px: f32) -> Self {
        let line = font
            .horizontal_line_metrics(px)
            .expect("monospace fonts have horizontal metrics");
        let cell_width = font.metrics('M', px).advance_width.ceil().max(1.0) as u32;
        Self {
            cell_width,
            cell_height: line.new_line_size.ceil().max(1.0) as u32,
            baseline: line.ascent.ceil() as i32,
            font,
            fallbacks,
            px,
            glyphs: HashMap::new(),
        }
    }

    pub fn with_px(self, px: f32) -> Self {
        Self::with_fallbacks(self.font, self.fallbacks, px)
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
            &self.font
        } else {
            self.fallbacks
                .get_or_insert_with(load_fallbacks)
                .iter()
                .find(|f| f.lookup_glyph_index(ch) != 0)
                .unwrap_or(&self.font)
        };
        let (metrics, coverage) = font.rasterize(ch, px);
        Glyph { metrics, coverage }
    }
}
