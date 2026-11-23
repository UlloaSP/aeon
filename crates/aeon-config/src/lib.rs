//! User configuration, read once at startup from `%APPDATA%\aeon\config.toml`.
//!
//! Every field has a default, so a missing file or a partial file is valid.
//! Unknown keys are rejected: a typo should fail loudly, not silently fall
//! back to a default.

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
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Shell {
    /// Program to launch. `None` picks the best installed shell.
    pub program: Option<String>,
    pub args: Vec<String>,
    /// Starting directory. `None` keeps the directory Aeon was launched from,
    /// or the home directory when launched from the Start menu.
    pub cwd: Option<PathBuf>,
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
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            foreground: Rgb::new(0xd8, 0xdb, 0xe2),
            background: Rgb::new(0x10, 0x11, 0x14),
            cursor: Rgb::new(0xf2, 0xb8, 0x4b),
        }
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
    fn missing_file_is_default() {
        let path = Path::new("definitely/not/here/config.toml");
        assert_eq!(Config::load(path).unwrap(), Config::default());
    }
}
