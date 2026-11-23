//! Built-in color themes. `[colors] theme` picks one; any color set next to
//! it in `[colors]` overrides that color of the theme.

use crate::Rgb;

pub struct Preset {
    /// What `[colors] theme` says.
    pub id: &'static str,
    /// What the settings panel shows.
    pub name: &'static str,
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    /// The bar and sidebar; `None` derives it from the background.
    pub bar_background: Option<Rgb>,
    /// ANSI 0-7, then their bright variants 8-15.
    pub ansi: [Rgb; 16],
}

const fn hex(rgb: u32) -> Rgb {
    Rgb::new((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

const fn ansi(colors: [u32; 16]) -> [Rgb; 16] {
    let mut out = [Rgb::new(0, 0, 0); 16];
    let mut i = 0;
    while i < 16 {
        out[i] = hex(colors[i]);
        i += 1;
    }
    out
}

pub const PRESETS: &[Preset] = &[
    // Aeon's own: the brand's ink, paper, and amber, with colors warm and
    // high contrast on the near-black ground, bright ones readable, not neon.
    Preset {
        id: "aeon",
        name: "Aeon",
        foreground: hex(0xf2f1ec),
        background: hex(0x0c0d10),
        cursor: hex(0xffb547),
        bar_background: Some(hex(0x17181c)),
        ansi: ansi([
            0x1d1f25, 0xe06c75, 0x8fc77f, 0xe8b75f, 0x61a6f2, 0xc38ae6, 0x4fc1c9, 0xc8ccd6,
            0x4b4f5c, 0xff7a85, 0xa6e095, 0xf6cd7d, 0x82bcff, 0xd7a6f7, 0x72d9e0, 0xf2f4f8,
        ]),
    },
    // The same identity on paper: colors darkened until they read on white.
    Preset {
        id: "aeon-light",
        name: "Aeon Light",
        foreground: hex(0x1c1d21),
        background: hex(0xf4f3ef),
        cursor: hex(0xb86e00),
        bar_background: None,
        ansi: ansi([
            0x1c1d21, 0xc4314b, 0x2f7d32, 0x8a6100, 0x1f62c8, 0x8e3fb5, 0x0f7f86, 0xa8a9ae,
            0x5b5d66, 0xe0485f, 0x3c9b40, 0xb07d00, 0x3a7ae0, 0xa85ad0, 0x1597a0, 0x6e7078,
        ]),
    },
    Preset {
        id: "dracula",
        name: "Dracula",
        foreground: hex(0xf8f8f2),
        background: hex(0x282a36),
        cursor: hex(0xf8f8f2),
        bar_background: None,
        ansi: ansi([
            0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2,
            0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff,
        ]),
    },
    Preset {
        id: "nord",
        name: "Nord",
        foreground: hex(0xd8dee9),
        background: hex(0x2e3440),
        cursor: hex(0xd8dee9),
        bar_background: None,
        ansi: ansi([
            0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0,
            0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
        ]),
    },
    Preset {
        id: "gruvbox-dark",
        name: "Gruvbox Dark",
        foreground: hex(0xebdbb2),
        background: hex(0x282828),
        cursor: hex(0xebdbb2),
        bar_background: None,
        ansi: ansi([
            0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
            0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
        ]),
    },
    Preset {
        id: "solarized-dark",
        name: "Solarized Dark",
        foreground: hex(0x839496),
        background: hex(0x002b36),
        cursor: hex(0x93a1a1),
        bar_background: None,
        ansi: ansi([
            0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
            0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
        ]),
    },
    Preset {
        id: "solarized-light",
        name: "Solarized Light",
        foreground: hex(0x657b83),
        background: hex(0xfdf6e3),
        cursor: hex(0x586e75),
        bar_background: None,
        ansi: ansi([
            0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
            0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
        ]),
    },
    Preset {
        id: "catppuccin-mocha",
        name: "Catppuccin Mocha",
        foreground: hex(0xcdd6f4),
        background: hex(0x1e1e2e),
        cursor: hex(0xf5e0dc),
        bar_background: None,
        ansi: ansi([
            0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de,
            0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
        ]),
    },
    Preset {
        id: "one-dark",
        name: "One Dark",
        foreground: hex(0xabb2bf),
        background: hex(0x282c34),
        cursor: hex(0x528bff),
        bar_background: None,
        ansi: ansi([
            0x282c34, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
            0x5c6370, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xffffff,
        ]),
    },
    Preset {
        id: "tokyo-night",
        name: "Tokyo Night",
        foreground: hex(0xc0caf5),
        background: hex(0x1a1b26),
        cursor: hex(0xc0caf5),
        bar_background: None,
        ansi: ansi([
            0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
            0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
        ]),
    },
];

pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_aeon_comes_first() {
        assert_eq!(PRESETS[0].id, "aeon");
        for (i, a) in PRESETS.iter().enumerate() {
            assert!(PRESETS[i + 1..].iter().all(|b| b.id != a.id), "{}", a.id);
        }
    }

    /// Relative luminance, per WCAG.
    fn luminance(c: Rgb) -> f64 {
        let channel = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
    }

    #[test]
    fn aeon_themes_keep_text_readable() {
        for id in ["aeon", "aeon-light"] {
            let p = preset(id).unwrap();
            let (a, b) = (luminance(p.foreground), luminance(p.background));
            let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
            assert!(ratio >= 7.0, "{id}: {ratio:.1}:1");
            // The six hues must stay legible on the background too.
            for (i, color) in p.ansi[1..7].iter().enumerate() {
                let c = luminance(*color);
                let ratio = (c.max(b) + 0.05) / (c.min(b) + 0.05);
                assert!(ratio >= 3.0, "{id} color {}: {ratio:.1}:1", i + 1);
            }
        }
    }
}
