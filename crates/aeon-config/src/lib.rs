//! User configuration, read once at startup from `%APPDATA%\aeon\config.toml`.
//!
//! Every field has a default, so a missing file or a partial file is valid.
//! Unknown keys are rejected: a typo should fail loudly, not silently fall
//! back to a default.

pub mod edit;

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::{self, Deserializer};

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub shell: Shell,
    pub font: Font,
    pub window: Window,
    pub colors: Colors,
    pub bar: Bar,
    pub launcher: Launcher,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Shell {
    /// Program to launch. `None` picks the best installed shell.
    pub program: Option<String>,
    pub args: Vec<String>,
    /// Starting directory. `None` keeps the directory Aeon was launched from,
    /// or the home directory when launched from the Start menu.
    pub cwd: Option<PathBuf>,
    /// Lets Aeon set up PowerShell and cmd: no user profile, Aeon's own
    /// history and prompt, and working-directory reporting for the bar.
    pub integration: bool,
}

impl Default for Shell {
    fn default() -> Self {
        Self {
            program: None,
            args: Vec::new(),
            cwd: None,
            integration: true,
        }
    }
}

/// The context bar under the terminal: directory, git, and running servers.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bar {
    pub enabled: bool,
}

impl Default for Bar {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// The quick launcher that the resident Aeon (`aeon --background`, started
/// at sign-in) opens from anywhere.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Launcher {
    /// Global hotkey, such as `"Ctrl+Alt+Space"` or `"Win+`"`.
    pub hotkey: String,
    /// Web search for queries that are not a command or an app. `{query}`
    /// marks where the words go; empty turns web search off.
    pub search_url: String,
}

impl Default for Launcher {
    fn default() -> Self {
        Self {
            hotkey: "Ctrl+Alt+Space".to_owned(),
            search_url: "https://www.google.com/search?q={query}".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Font {
    /// Path to a TrueType/OpenType file. `None` uses the system monospace font.
    pub path: Option<PathBuf>,
    /// Size in points at 100% display scale.
    pub size: f32,
}

impl Default for Font {
    fn default() -> Self {
        Self {
            path: None,
            size: 11.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Window {
    pub columns: u16,
    pub rows: u16,
    /// Inner padding in logical pixels.
    pub padding: u16,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            columns: 120,
            rows: 32,
            padding: 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Colors {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub bar_background: Rgb,
    /// ANSI colors 0-7. Unset entries keep Aeon's theme.
    pub normal: Palette,
    /// ANSI colors 8-15. Unset entries keep Aeon's theme.
    pub bright: Palette,
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            // Aeon's brand: ink, paper, and the amber of the logo's cursor.
            foreground: Rgb::new(0xf2, 0xf1, 0xec),
            background: Rgb::new(0x0c, 0x0d, 0x10),
            cursor: Rgb::new(0xff, 0xb5, 0x47),
            bar_background: Rgb::new(0x17, 0x18, 0x1c),
            normal: Palette::default(),
            bright: Palette::default(),
        }
    }
}

/// Aeon's theme: warm, high contrast on the near-black background, with
/// bright variants that stay readable rather than neon.
const AEON_ANSI: [Rgb; 16] = [
    Rgb::new(0x1d, 0x1f, 0x25), // black
    Rgb::new(0xe0, 0x6c, 0x75), // red
    Rgb::new(0x8f, 0xc7, 0x7f), // green
    Rgb::new(0xe8, 0xb7, 0x5f), // yellow
    Rgb::new(0x61, 0xa6, 0xf2), // blue
    Rgb::new(0xc3, 0x8a, 0xe6), // magenta
    Rgb::new(0x4f, 0xc1, 0xc9), // cyan
    Rgb::new(0xc8, 0xcc, 0xd6), // white
    Rgb::new(0x4b, 0x4f, 0x5c), // bright black
    Rgb::new(0xff, 0x7a, 0x85), // bright red
    Rgb::new(0xa6, 0xe0, 0x95), // bright green
    Rgb::new(0xf6, 0xcd, 0x7d), // bright yellow
    Rgb::new(0x82, 0xbc, 0xff), // bright blue
    Rgb::new(0xd7, 0xa6, 0xf7), // bright magenta
    Rgb::new(0x72, 0xd9, 0xe0), // bright cyan
    Rgb::new(0xf2, 0xf4, 0xf8), // bright white
];

impl Colors {
    /// The 16 ANSI colors with the user's overrides applied.
    pub fn ansi(&self) -> [Rgb; 16] {
        let mut ansi = AEON_ANSI;
        for (offset, palette) in [(0, &self.normal), (8, &self.bright)] {
            for (i, color) in palette.entries().into_iter().enumerate() {
                if let Some(color) = color {
                    ansi[offset + i] = color;
                }
            }
        }
        ansi
    }
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Palette {
    pub black: Option<Rgb>,
    pub red: Option<Rgb>,
    pub green: Option<Rgb>,
    pub yellow: Option<Rgb>,
    pub blue: Option<Rgb>,
    pub magenta: Option<Rgb>,
    pub cyan: Option<Rgb>,
    pub white: Option<Rgb>,
}

impl Palette {
    fn entries(&self) -> [Option<Rgb>; 8] {
        [
            self.black,
            self.red,
            self.green,
            self.yellow,
            self.blue,
            self.magenta,
            self.cyan,
            self.white,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// `0x00RRGGBB`, the pixel format softbuffer expects.
    pub const fn to_u32(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | self.b as u32
    }
}

impl std::str::FromStr for Rgb {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s
            .strip_prefix('#')
            .filter(|h| h.len() == 6 && h.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| format!("expected a color like \"#1e1e2e\", got {s:?}"))?;
        let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string());
        Ok(Self::new(channel(0)?, channel(2)?, channel(4)?))
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(de::Error::custom)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid config {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
}

impl Config {
    /// `%APPDATA%\aeon\config.toml`. `None` when the platform has no config dir.
    pub fn default_path() -> Option<PathBuf> {
        let base = std::env::var_os("APPDATA").or_else(|| std::env::var_os("XDG_CONFIG_HOME"))?;
        Some(PathBuf::from(base).join("aeon").join("config.toml"))
    }

    /// Loads `path`, or the defaults when it does not exist.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).map_err(|source| ConfigError::Parse {
                path: path.to_owned(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Read {
                path: path.to_owned(),
                source,
            }),
        }
    }

    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_default() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let config = Config::parse(
            r##"
            [font]
            size = 14.0

            [colors]
            background = "#000000"
            "##,
        )
        .unwrap();
        assert_eq!(config.font.size, 14.0);
        assert_eq!(config.colors.background, Rgb::new(0, 0, 0));
        assert_eq!(config.colors.foreground, Colors::default().foreground);
        assert_eq!(config.window, Window::default());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(Config::parse("[font]\nsize = 12.0\nfamliy = \"x\"").is_err());
    }

    #[test]
    fn rgb_round_trips() {
        let rgb: Rgb = "#1E1e2e".parse().unwrap();
        assert_eq!(rgb, Rgb::new(0x1e, 0x1e, 0x2e));
        assert_eq!(rgb.to_string(), "#1e1e2e");
        assert_eq!(rgb.to_u32(), 0x001e_1e2e);
    }

    #[test]
    fn rgb_rejects_malformed() {
        for bad in ["1e1e2e", "#1e1e2", "#1e1e2g", "#1e1e2e0", ""] {
            assert!(bad.parse::<Rgb>().is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn palette_overrides_only_what_is_set() {
        let config = Config::parse(
            "[colors.bright]
red = \"#ff0000\"",
        )
        .unwrap();
        let ansi = config.colors.ansi();
        assert_eq!(ansi[9], Rgb::new(0xff, 0, 0));
        assert_eq!(ansi[1], AEON_ANSI[1]);
        assert_eq!(ansi[10], AEON_ANSI[10]);
    }

    #[test]
    fn shell_integration_and_bar_default_on() {
        let config = Config::parse(
            "[shell]
program = \"cmd.exe\"",
        )
        .unwrap();
        assert!(config.shell.integration);
        assert!(config.bar.enabled);
    }

    #[test]
    fn missing_file_is_default() {
        let path = Path::new("definitely/not/here/config.toml");
        assert_eq!(Config::load(path).unwrap(), Config::default());
    }
}
