use std::collections::HashMap;
use std::path::{Path, PathBuf};

use fontdue::{Font, FontSettings, Metrics};

/// Tried in order when the config names no font.
const SYSTEM_FONTS: &[&str] = &[
    r"C:\Windows\Fonts\CascadiaMono.ttf",
    r"C:\Windows\Fonts\consola.ttf",
    r"C:\Windows\Fonts\lucon.ttf",
];

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

pub struct Glyph {
    pub metrics: Metrics,
    pub coverage: Vec<u8>,
}

/// One font at one pixel size, with every glyph rasterized at most once.
pub struct Atlas {
    font: Font,
    px: f32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub baseline: i32,
    glyphs: HashMap<char, Glyph>,
}

impl Atlas {
    pub fn new(font: Font, px: f32) -> Self {
        let line = font
            .horizontal_line_metrics(px)
            .expect("monospace fonts have horizontal metrics");
        let cell_width = font.metrics('M', px).advance_width.ceil().max(1.0) as u32;
        Self {
            cell_width,
            cell_height: line.new_line_size.ceil().max(1.0) as u32,
            baseline: line.ascent.ceil() as i32,
            font,
            px,
            glyphs: HashMap::new(),
        }
    }

    pub fn with_px(self, px: f32) -> Self {
        Self::new(self.font, px)
    }

    pub fn glyph(&mut self, ch: char) -> &Glyph {
        let (font, px) = (&self.font, self.px);
        self.glyphs.entry(ch).or_insert_with(|| {
            let (metrics, coverage) = font.rasterize(ch, px);
            Glyph { metrics, coverage }
        })
    }
}
