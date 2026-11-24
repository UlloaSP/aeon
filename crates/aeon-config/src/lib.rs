//! User configuration, read once at startup from `%APPDATA%\aeon\config.toml`.
//!
//! Every field has a default, so a missing file or a partial file is valid.
//! Unknown keys are rejected: a typo should fail loudly, not silently fall
//! back to a default.

pub mod edit;
pub mod themes;

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
    pub keys: Keys,
    pub cursor: Cursor,
    pub updates: Updates,
    pub terminal: TerminalSettings,
}

/// How the terminal itself behaves.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalSettings {
    /// Lines kept above the screen for scrolling back. 0 keeps none.
    pub scrollback: usize,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self { scrollback: 5000 }
    }
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
    /// Lets Aeon set up the shell: its prompt, command blocks, history, and
    /// working-directory reporting for the header.
    pub integration: bool,
    /// With integration, PowerShell still loads the user's profiles first.
    /// Off starts faster, without their modules and PATH changes.
    pub profile: bool,
}

impl Default for Shell {
    fn default() -> Self {
        Self {
            program: None,
            args: Vec::new(),
            cwd: None,
            integration: true,
            profile: true,
        }
    }
}

/// The session header above the terminal: project, git, and running servers.
/// The key keeps its old name, `[bar]`, so existing configs still load.
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
    /// Global hotkey for the drop-down terminal; empty turns it off.
    pub dropdown: String,
    /// Web search for queries that are not a command or an app. `{query}`
    /// marks where the words go; empty turns web search off.
    pub search_url: String,
    /// Whether the resident starts at sign-in. The installer's startup entry
    /// always runs; the resident exits at once when this is off.
    pub start_with_windows: bool,
}

impl Default for Launcher {
    fn default() -> Self {
        Self {
            hotkey: "Ctrl+Alt+Space".to_owned(),
            dropdown: "Ctrl+Alt+`".to_owned(),
            search_url: "https://www.google.com/search?q={query}".to_owned(),
            start_with_windows: true,
        }
    }
}

/// Aeon's own shortcuts in the terminal window, as `"Ctrl+Shift+T"`. The
/// defaults use Ctrl+Shift because shells already use Ctrl+T, Ctrl+W, Ctrl+B.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Keys {
    pub new_session: String,
    pub close_session: String,
    pub next_session: String,
    pub previous_session: String,
    pub toggle_sidebar: String,
    pub settings: String,
    pub find: String,
    pub copy: String,
    pub paste: String,
    pub project_panel: String,
    pub split_right: String,
    pub split_down: String,
    pub zoom_pane: String,
    pub rename_session: String,
    pub palette: String,
}

impl Default for Keys {
    fn default() -> Self {
        Self {
            new_session: "Ctrl+Shift+T".to_owned(),
            close_session: "Ctrl+Shift+W".to_owned(),
            next_session: "Ctrl+Tab".to_owned(),
            previous_session: "Ctrl+Shift+Tab".to_owned(),
            toggle_sidebar: "Ctrl+Shift+B".to_owned(),
            settings: "Ctrl+,".to_owned(),
            find: "Ctrl+Shift+F".to_owned(),
            copy: "Ctrl+Shift+C".to_owned(),
            paste: "Ctrl+Shift+V".to_owned(),
            project_panel: "Ctrl+Shift+P".to_owned(),
            split_right: "Ctrl+Shift+D".to_owned(),
            split_down: "Ctrl+Shift+E".to_owned(),
            zoom_pane: "Ctrl+Shift+Z".to_owned(),
            rename_session: "F2".to_owned(),
            palette: "Ctrl+Shift+K".to_owned(),
        }
    }
}

/// Updates, which the resident checks for and the user installs.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Updates {
    pub channel: Channel,
    /// Check shortly after sign-in and every few hours. Off, Aeon only checks
    /// when asked.
    pub check: bool,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            channel: Channel::Stable,
            check: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    #[default]
    Stable,
    /// Daily builds of `develop`, and stable releases when they are newer.
    Nightly,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cursor {
    pub style: CursorStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    #[default]
    Block,
    Bar,
    Underline,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Font {
    /// Path to a TrueType/OpenType file. `None` uses the system monospace font.
    pub path: Option<PathBuf>,
    /// Size in points at 100% display scale.
    pub size: f32,
    /// Row height as a multiple of the font's own line height.
    pub line_height: f32,
}

impl Default for Font {
    fn default() -> Self {
        Self {
            path: None,
            size: 11.0,
            line_height: 1.2,
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
    /// Reopen the last window's sessions when Aeon starts with none open.
    pub restore: bool,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            columns: 120,
            rows: 32,
            padding: 8,
            restore: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Colors {
    /// Built-in theme the colors start from; see [`themes::PRESETS`].
    #[serde(deserialize_with = "theme_id")]
    pub theme: String,
    /// Each set color overrides the theme's.
    pub foreground: Option<Rgb>,
    pub background: Option<Rgb>,
    pub cursor: Option<Rgb>,
    /// The sidebar and other raised surfaces. Unset: a step from the background toward
    /// the foreground.
    pub bar_background: Option<Rgb>,
    /// ANSI colors 0-7. Unset entries keep the theme's.
    pub normal: Palette,
    /// ANSI colors 8-15. Unset entries keep the theme's.
    pub bright: Palette,
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            theme: "aeon".to_owned(),
            foreground: None,
            background: None,
            cursor: None,
            bar_background: None,
            normal: Palette::default(),
            bright: Palette::default(),
        }
    }
}

fn theme_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let id = String::deserialize(deserializer)?;
    if themes::preset(&id).is_some() {
        return Ok(id);
    }
    let known: Vec<&str> = themes::PRESETS.iter().map(|p| p.id).collect();
    Err(de::Error::custom(format!(
        "unknown theme {id:?}, expected one of: {}",
        known.join(", ")
    )))
}

/// The colors to paint with: the theme with the user's overrides applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resolved {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub bar_background: Rgb,
    pub ansi: [Rgb; 16],
}

impl Colors {
    pub fn resolve(&self) -> Resolved {
        let preset = themes::preset(&self.theme).unwrap_or(&themes::PRESETS[0]);
        let mut ansi = preset.ansi;
        for (offset, palette) in [(0, &self.normal), (8, &self.bright)] {
            for (i, color) in palette.entries().into_iter().enumerate() {
                if let Some(color) = color {
                    ansi[offset + i] = color;
                }
            }
        }
        let foreground = self.foreground.unwrap_or(preset.foreground);
        let background = self.background.unwrap_or(preset.background);
        Resolved {
            foreground,
            background,
            cursor: self.cursor.unwrap_or(preset.cursor),
            bar_background: self
                .bar_background
                .or(preset.bar_background.filter(|_| self.background.is_none()))
                .unwrap_or_else(|| background.mix(foreground, 0.05)),
            ansi,
        }
    }

    /// The 16 ANSI colors with the user's overrides applied.
    pub fn ansi(&self) -> [Rgb; 16] {
        self.resolve().ansi
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

    /// `amount` (0 to 1) of the way from `self` to `other`.
    pub fn mix(self, other: Rgb, amount: f32) -> Rgb {
        let m =
            |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * amount).round() as u8;
        Rgb::new(m(self.r, other.r), m(self.g, other.g), m(self.b, other.b))
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
        assert_eq!(config.colors.resolve().background, Rgb::new(0, 0, 0));
        assert_eq!(
            config.colors.resolve().foreground,
            Colors::default().resolve().foreground
        );
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
        assert_eq!(ansi[1], themes::PRESETS[0].ansi[1]);
        assert_eq!(ansi[10], themes::PRESETS[0].ansi[10]);
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
    fn themes_resolve_with_overrides() {
        let config = Config::parse("[colors]\ntheme = \"dracula\"\ncursor = \"#ff0000\"").unwrap();
        let colors = config.colors.resolve();
        assert_eq!(colors.background, Rgb::new(0x28, 0x2a, 0x36));
        assert_eq!(colors.cursor, Rgb::new(0xff, 0, 0));
        // The bar is derived from the theme when not set.
        assert_ne!(colors.bar_background, colors.background);
    }

    #[test]
    fn unknown_themes_are_an_error() {
        let err = Config::parse("[colors]\ntheme = \"draculaa\"")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("unknown theme") && err.contains("dracula"),
            "{err}"
        );
    }

    #[test]
    fn aeon_bar_matches_the_brand() {
        assert_eq!(
            Colors::default().resolve().bar_background,
            Rgb::new(0x17, 0x18, 0x1c)
        );
    }

    #[test]
    fn keys_and_cursor_style_parse() {
        let config =
            Config::parse("[keys]\nnew_session = \"Ctrl+N\"\n[cursor]\nstyle = \"bar\"").unwrap();
        assert_eq!(config.keys.new_session, "Ctrl+N");
        assert_eq!(config.keys.settings, "Ctrl+,");
        assert_eq!(config.keys.find, "Ctrl+Shift+F");
        assert_eq!(Config::default().terminal.scrollback, 5000);
        assert_eq!(
            Config::parse("[terminal]\nscrollback = 0")
                .unwrap()
                .terminal
                .scrollback,
            0
        );
        assert_eq!(config.cursor.style, CursorStyle::Bar);
        assert!(Config::parse("[cursor]\nstyle = \"beam\"").is_err());
    }

    #[test]
    fn updates_default_to_stable_with_checks() {
        assert_eq!(Config::default().updates.channel, Channel::Stable);
        assert!(Config::default().updates.check);
        let config = Config::parse("[updates]\nchannel = \"nightly\"\ncheck = false").unwrap();
        assert_eq!(config.updates.channel, Channel::Nightly);
        assert!(!config.updates.check);
        assert!(Config::parse("[updates]\nchannel = \"beta\"").is_err());
    }

    #[test]
    fn missing_file_is_default() {
        let path = Path::new("definitely/not/here/config.toml");
        assert_eq!(Config::load(path).unwrap(), Config::default());
    }
}
