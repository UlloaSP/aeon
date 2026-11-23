//! The settings panel: sections on the left, rows on the right, each row a
//! setting saved straight to config.toml (comments kept, see
//! `aeon_config::edit`). It takes the place of the terminal while open.

use std::path::{Path, PathBuf};

use aeon_config::Config;
use aeon_config::edit::{Change, Value};
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::font::Atlas;
use crate::render::{self, Frame, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    General,
    Appearance,
    Launcher,
    About,
}

const SECTIONS: [(Section, &str); 4] = [
    (Section::General, "General"),
    (Section::Appearance, "Appearance"),
    (Section::Launcher, "Launcher"),
    (Section::About, "About"),
];

/// One value of a multiple-choice setting and what picking it writes.
#[derive(Debug, Clone)]
struct Choice {
    label: String,
    changes: Vec<Change>,
}

#[derive(Debug, Clone)]
enum Control {
    Toggle {
        on: bool,
        table: &'static str,
        key: &'static str,
    },
    Choice {
        options: Vec<Choice>,
        index: usize,
    },
    Number {
        value: f64,
        step: f64,
        min: f64,
        max: f64,
        unit: &'static str,
        table: &'static str,
        key: &'static str,
        integer: bool,
    },
    Hotkey {
        value: String,
    },
    Info(String),
    Button(&'static str),
}

#[derive(Debug, Clone)]
struct Row {
    title: &'static str,
    description: &'static str,
    control: Control,
}

/// What the app must do after an input.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    None,
    Redraw,
    Save(Vec<Change>),
    OpenConfigFile,
    Close,
}

pub struct Settings {
    section: usize,
    selected: usize,
    recording: bool,
    rows: Vec<Row>,
    shells: Vec<Choice>,
    fonts: Vec<Choice>,
    /// Shown under the rows: why the last save failed.
    pub error: Option<String>,
}

impl Settings {
    pub fn new(config: &Config) -> Self {
        let mut settings = Self {
            section: 0,
            selected: 0,
            recording: false,
            rows: Vec::new(),
            shells: detect_shells(),
            fonts: detect_fonts(),
            error: None,
        };
        settings.rebuild(config);
        settings
    }

    /// Rows for the current section, from what the config says now.
    pub fn rebuild(&mut self, config: &Config) {
        self.rows = rows(SECTIONS[self.section].0, config, &self.shells, &self.fonts);
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    fn select_section(&mut self, section: usize, config: &Config) {
        self.section = section % SECTIONS.len();
        self.selected = 0;
        self.recording = false;
        self.rebuild(config);
    }

    pub fn key(&mut self, key: &Key, mods: ModifiersState, config: &Config) -> Outcome {
        if self.recording {
            return self.record(key, mods);
        }
        match key {
            Key::Named(NamedKey::Escape) => Outcome::Close,
            Key::Named(NamedKey::Tab) => {
                let step = if mods.shift_key() {
                    SECTIONS.len() - 1
                } else {
                    1
                };
                self.select_section(self.section + step, config);
                Outcome::Redraw
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.selected = (self.selected + 1).min(self.rows.len().saturating_sub(1));
                Outcome::Redraw
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.selected = self.selected.saturating_sub(1);
                Outcome::Redraw
            }
            Key::Named(NamedKey::ArrowLeft) => self.change(-1),
            Key::Named(NamedKey::ArrowRight) => self.change(1),
            Key::Named(NamedKey::Enter | NamedKey::Space) => self.activate(),
            _ => Outcome::None,
        }
    }

    /// Steps the selected row's value by `delta` (-1 or 1).
    fn change(&mut self, delta: i32) -> Outcome {
        let Some(row) = self.rows.get(self.selected) else {
            return Outcome::None;
        };
        match &row.control {
            Control::Toggle { on, table, key } => {
                // Left means off and right means on, like the switch looks.
                let wanted = delta > 0;
                if *on == wanted {
                    return Outcome::None;
                }
                Outcome::Save(vec![change(table, key, Some(Value::Bool(wanted)))])
            }
            Control::Choice { options, index } if !options.is_empty() => {
                let n = options.len() as i32;
                let next = (*index as i32 + delta).rem_euclid(n) as usize;
                Outcome::Save(options[next].changes.clone())
            }
            Control::Number {
                value,
                step,
                min,
                max,
                table,
                key,
                integer,
                ..
            } => {
                let next = (value + step * f64::from(delta)).clamp(*min, *max);
                if (next - value).abs() < f64::EPSILON {
                    return Outcome::None;
                }
                let v = if *integer {
                    Value::Int(next.round() as i64)
                } else {
                    Value::Float(next)
                };
                Outcome::Save(vec![change(table, key, Some(v))])
            }
            _ => Outcome::None,
        }
    }

    fn activate(&mut self) -> Outcome {
        let Some(row) = self.rows.get(self.selected) else {
            return Outcome::None;
        };
        match &row.control {
            Control::Toggle { on, table, key } => {
                Outcome::Save(vec![change(table, key, Some(Value::Bool(!on)))])
            }
            Control::Choice { .. } => self.change(1),
            Control::Hotkey { .. } => {
                self.recording = true;
                self.error = None;
                Outcome::Redraw
            }
            Control::Button(_) => Outcome::OpenConfigFile,
            _ => Outcome::None,
        }
    }

    /// While recording, the next key with at least one modifier becomes the
    /// hotkey. Escape cancels.
    fn record(&mut self, key: &Key, mods: ModifiersState) -> Outcome {
        if let Key::Named(NamedKey::Escape) = key {
            self.recording = false;
            return Outcome::Redraw;
        }
        let Some(name) = hotkey_key(key) else {
            return Outcome::None; // a modifier alone: keep waiting
        };
        let mut text = String::new();
        for (on, label) in [
            (mods.control_key(), "Ctrl+"),
            (mods.alt_key(), "Alt+"),
            (mods.shift_key(), "Shift+"),
            (mods.super_key(), "Win+"),
        ] {
            if on {
                text.push_str(label);
            }
        }
        if text.is_empty() {
            self.error = Some("A hotkey needs Ctrl, Alt, Shift, or Win.".into());
            return Outcome::Redraw;
        }
        text.push_str(&name);
        self.recording = false;
        Outcome::Save(vec![change("launcher", "hotkey", Some(Value::Str(text)))])
    }

    pub fn click(&mut self, x: f64, y: f64, geo: &Geometry, config: &Config) -> Outcome {
        if x < f64::from(geo.left) {
            return Outcome::None;
        }
        if x < f64::from(geo.left + geo.nav) {
            let Some(i) = geo.nav_item_at(y) else {
                return Outcome::None;
            };
            if i < SECTIONS.len() {
                self.select_section(i, config);
                return Outcome::Redraw;
            }
            return Outcome::None;
        }
        let Some(i) = geo.row_at(y, self.rows.len()) else {
            return Outcome::None;
        };
        self.selected = i;
        self.recording = false;
        if x < f64::from(geo.control_x) {
            return Outcome::Redraw;
        }
        match self.rows[i].control {
            Control::Choice { .. } | Control::Number { .. } => {
                // The left half steps back, the right half forward.
                let middle = f64::from(geo.control_x + geo.control_w / 2);
                self.change(if x < middle { -1 } else { 1 })
            }
            _ => self.activate(),
        }
    }

    pub fn draw(&self, frame: &mut Frame<'_>, atlas: &mut Atlas, theme: &Theme, geo: &Geometry) {
        let muted = theme.ansi[8];
        let (cw, ch) = (atlas.cell_width, atlas.cell_height);
        let content_w = frame.width.saturating_sub(geo.left);
        render::fill_rect(frame, geo.left, 0, content_w, frame.height, theme.bg);
        render::fill_rect(frame, geo.left, 0, geo.nav, frame.height, theme.bar_bg);

        for (i, (_, name)) in SECTIONS.iter().enumerate() {
            let top = geo.nav_top + i as u32 * geo.nav_row;
            if i == self.section {
                let highlight = render::blend(theme.bar_bg, theme.fg, 22);
                render::fill_rect(frame, geo.left, top, geo.nav, geo.nav_row, highlight);
                render::fill_rect(
                    frame,
                    geo.left,
                    top,
                    (geo.pad / 3).max(2),
                    geo.nav_row,
                    theme.cursor,
                );
            }
            let color = if i == self.section { theme.fg } else { muted };
            let y = top + (geo.nav_row - ch) / 2;
            render::draw_text(
                frame,
                atlas,
                geo.left + geo.pad,
                y,
                name,
                color,
                ((geo.nav - 2 * geo.pad) / cw) as usize,
            );
        }

        let x = geo.left + geo.nav + geo.pad * 2;
        let title = SECTIONS[self.section].1;
        render::draw_text(frame, atlas, x, geo.pad * 2, title, theme.fg, 40);

        let title_cols = ((geo.control_x.saturating_sub(x + geo.pad)) / cw) as usize;
        for (i, row) in self.rows.iter().enumerate() {
            let top = geo.rows_top + i as u32 * geo.row;
            if top + geo.row > frame.height {
                break;
            }
            if i == self.selected {
                let highlight = render::blend(theme.bg, theme.fg, 18);
                render::fill_rect(
                    frame,
                    x - geo.pad,
                    top,
                    frame.width.saturating_sub(x),
                    geo.row,
                    highlight,
                );
            }
            let line1 = top + geo.pad;
            render::draw_text(frame, atlas, x, line1, row.title, theme.fg, title_cols);
            render::draw_text(
                frame,
                atlas,
                x,
                line1 + ch,
                row.description,
                muted,
                title_cols,
            );
            self.draw_control(
                frame,
                atlas,
                theme,
                geo,
                &row.control,
                line1 + ch / 2,
                i == self.selected,
            );
        }

        let hint = if self.recording {
            "Press the new hotkey \u{b7} Esc cancels"
        } else {
            "\u{2191}\u{2193} move \u{b7} \u{2190}\u{2192} change \u{b7} Enter toggle \u{b7} Tab section \u{b7} Esc close"
        };
        let bottom = frame.height.saturating_sub(geo.pad + ch);
        render::draw_text(
            frame,
            atlas,
            x,
            bottom,
            hint,
            muted,
            ((frame.width - x) / cw) as usize,
        );
        if let Some(error) = &self.error {
            let [_, red, ..] = theme.ansi;
            render::draw_text(
                frame,
                atlas,
                x,
                bottom.saturating_sub(ch + geo.pad),
                error,
                red,
                ((frame.width - x) / cw) as usize,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_control(
        &self,
        frame: &mut Frame<'_>,
        atlas: &mut Atlas,
        theme: &Theme,
        geo: &Geometry,
        control: &Control,
        y: u32,
        selected: bool,
    ) {
        let muted = theme.ansi[8];
        let [_, _, _, _, blue, ..] = theme.ansi;
        let cw = atlas.cell_width;
        let cols = (geo.control_w / cw) as usize;
        // Right-aligned text inside the control column.
        let right =
            |text: &str| geo.control_x + geo.control_w - text.chars().count().min(cols) as u32 * cw;
        match control {
            Control::Toggle { on, .. } => {
                let h = atlas.cell_height;
                let w = h * 2;
                let x = geo.control_x + geo.control_w - w;
                let track = if *on {
                    theme.cursor
                } else {
                    render::blend(theme.bg, theme.fg, 60)
                };
                render::fill_rect(frame, x, y, w, h, track);
                let inset = (h / 6).max(1);
                let knob = h - 2 * inset;
                let kx = if *on { x + w - inset - knob } else { x + inset };
                render::fill_rect(frame, kx, y + inset, knob, knob, theme.bg);
            }
            Control::Choice { options, index } => {
                let label = options.get(*index).map_or("", |o| o.label.as_str());
                let text = format!("\u{2039} {label} \u{203a}");
                self.arrows(frame, atlas, theme, &text, right(&text), y, cols, selected);
            }
            Control::Number {
                value,
                unit,
                integer,
                ..
            } => {
                let number = if *integer || value.fract() == 0.0 {
                    format!("{value:.0}")
                } else {
                    format!("{value:.1}")
                };
                let text = format!("\u{2039} {number} {unit} \u{203a}");
                self.arrows(frame, atlas, theme, &text, right(&text), y, cols, selected);
            }
            Control::Hotkey { value } => {
                let text = if self.recording && selected {
                    "Press keys\u{2026}".to_owned()
                } else {
                    value.clone()
                };
                let color = if self.recording && selected {
                    theme.cursor
                } else {
                    theme.fg
                };
                render::draw_text(frame, atlas, right(&text), y, &text, color, cols);
            }
            Control::Info(text) => {
                render::draw_text(frame, atlas, right(text), y, text, muted, cols)
            }
            Control::Button(label) => {
                let text = format!("{label} \u{203a}");
                render::draw_text(frame, atlas, right(&text), y, &text, blue, cols);
            }
        }
    }

    /// A value between `‹ ›`, with the arrows dimmed unless the row is selected.
    #[allow(clippy::too_many_arguments)]
    fn arrows(
        &self,
        frame: &mut Frame<'_>,
        atlas: &mut Atlas,
        theme: &Theme,
        text: &str,
        x: u32,
        y: u32,
        cols: usize,
        selected: bool,
    ) {
        render::draw_text(frame, atlas, x, y, text, theme.fg, cols);
        let arrow = if selected {
            theme.cursor
        } else {
            theme.ansi[8]
        };
        let n = text.chars().count().min(cols) as u32;
        render::draw_glyph(frame, atlas, '\u{2039}', x as i32, y as i32, arrow);
        let last = x + (n.saturating_sub(1)) * atlas.cell_width;
        render::draw_glyph(frame, atlas, '\u{203a}', last as i32, y as i32, arrow);
    }
}

/// The panel's measurements, shared by drawing and click handling.
pub struct Geometry {
    /// First pixel column of the panel (right of the sidebar).
    left: u32,
    pad: u32,
    nav: u32,
    nav_top: u32,
    nav_row: u32,
    rows_top: u32,
    row: u32,
    control_x: u32,
    control_w: u32,
}

impl Geometry {
    pub fn new(left: u32, width: u32, atlas: &Atlas, scale: f64) -> Self {
        let px = |logical: f64| (logical * scale).round() as u32;
        let pad = px(10.0);
        let nav = px(170.0).min(width.saturating_sub(left) / 3);
        let control_w = px(240.0).min(width.saturating_sub(left + nav) / 2);
        Self {
            left,
            pad,
            nav,
            nav_top: pad * 2,
            nav_row: atlas.cell_height + 2 * pad,
            rows_top: pad * 4 + atlas.cell_height,
            row: 2 * atlas.cell_height + 2 * pad,
            control_x: width.saturating_sub(control_w + pad * 2),
            control_w,
        }
    }

    fn nav_item_at(&self, y: f64) -> Option<usize> {
        if y < f64::from(self.nav_top) {
            return None;
        }
        Some(((y - f64::from(self.nav_top)) / f64::from(self.nav_row)) as usize)
    }

    fn row_at(&self, y: f64, rows: usize) -> Option<usize> {
        if y < f64::from(self.rows_top) {
            return None;
        }
        let i = ((y - f64::from(self.rows_top)) / f64::from(self.row)) as usize;
        (i < rows).then_some(i)
    }
}

fn change(table: &'static str, key: &'static str, value: Option<Value>) -> Change {
    Change { table, key, value }
}

fn rows(section: Section, config: &Config, shells: &[Choice], fonts: &[Choice]) -> Vec<Row> {
    match section {
        Section::General => vec![
            Row {
                title: "Shell",
                description: "What new sessions run.",
                control: Control::Choice {
                    index: current_shell(config, shells),
                    options: shells.to_vec(),
                },
            },
            Row {
                title: "Shell integration",
                description: "Aeon's prompt and history. Off loads your PowerShell profile.",
                control: Control::Toggle {
                    on: config.shell.integration,
                    table: "shell",
                    key: "integration",
                },
            },
            Row {
                title: "Context bar",
                description: "Folder, git, and servers under the terminal.",
                control: Control::Toggle {
                    on: config.bar.enabled,
                    table: "bar",
                    key: "enabled",
                },
            },
        ],
        Section::Appearance => vec![
            Row {
                title: "Font",
                description: "Monospace fonts installed on this PC.",
                control: Control::Choice {
                    index: current_font(config, fonts),
                    options: fonts.to_vec(),
                },
            },
            Row {
                title: "Font size",
                description: "Text size at 100% display scale.",
                control: Control::Number {
                    value: f64::from(config.font.size),
                    step: 0.5,
                    min: 6.0,
                    max: 32.0,
                    unit: "pt",
                    table: "font",
                    key: "size",
                    integer: false,
                },
            },
            Row {
                title: "Padding",
                description: "Space around the terminal text.",
                control: Control::Number {
                    value: f64::from(config.window.padding),
                    step: 2.0,
                    min: 0.0,
                    max: 48.0,
                    unit: "px",
                    table: "window",
                    key: "padding",
                    integer: true,
                },
            },
        ],
        Section::Launcher => {
            let engines = search_engines();
            let index = engines
                .iter()
                .position(|(_, url)| *url == config.launcher.search_url);
            let mut options: Vec<Choice> = engines
                .iter()
                .map(|(label, url)| Choice {
                    label: (*label).to_owned(),
                    changes: vec![change(
                        "launcher",
                        "search_url",
                        Some(Value::Str((*url).to_owned())),
                    )],
                })
                .collect();
            // A hand-written URL stays selectable, so stepping away is undoable.
            let index = index.unwrap_or_else(|| {
                options.push(Choice {
                    label: "Custom".into(),
                    changes: vec![change(
                        "launcher",
                        "search_url",
                        Some(Value::Str(config.launcher.search_url.clone())),
                    )],
                });
                options.len() - 1
            });
            vec![
                Row {
                    title: "Hotkey",
                    description: "Opens the launcher from anywhere. Enter to record.",
                    control: Control::Hotkey {
                        value: config.launcher.hotkey.clone(),
                    },
                },
                Row {
                    title: "Web search",
                    description: "Where queries that are not a command or app go.",
                    control: Control::Choice { options, index },
                },
            ]
        }
        Section::About => vec![
            Row {
                title: "Version",
                description: "The Aeon build you are running.",
                control: Control::Info(crate::VERSION.to_owned()),
            },
            Row {
                title: "Configuration file",
                description: "Every setting, including ones not shown here.",
                control: Control::Button("Open"),
            },
        ],
    }
}

fn search_engines() -> [(&'static str, &'static str); 4] {
    [
        ("Google", "https://www.google.com/search?q={query}"),
        ("DuckDuckGo", "https://duckduckgo.com/?q={query}"),
        ("Bing", "https://www.bing.com/search?q={query}"),
        ("Off", ""),
    ]
}

fn current_shell(config: &Config, shells: &[Choice]) -> usize {
    let Some(program) = &config.shell.program else {
        return 0;
    };
    let stem = |p: &str| {
        Path::new(p)
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    };
    let wanted = stem(program);
    shells
        .iter()
        .position(|choice| {
            choice.changes.iter().any(|c| {
                matches!(&c.value, Some(Value::Str(p)) if c.key == "program" && stem(p) == wanted)
            })
        })
        .unwrap_or(0)
}

fn current_font(config: &Config, fonts: &[Choice]) -> usize {
    let Some(path) = &config.font.path else {
        return 0;
    };
    fonts
        .iter()
        .position(|choice| {
            choice
                .changes
                .iter()
                .any(|c| matches!(&c.value, Some(Value::Str(p)) if Path::new(p) == path.as_path()))
        })
        .unwrap_or(0)
}

/// Automatic first, then the shells this PC has.
fn detect_shells() -> Vec<Choice> {
    let set = |program: &str, args: &[&str]| {
        vec![
            change("shell", "program", Some(Value::Str(program.to_owned()))),
            change(
                "shell",
                "args",
                (!args.is_empty())
                    .then(|| Value::List(args.iter().map(|a| (*a).to_owned()).collect())),
            ),
        ]
    };
    let on_path = |exe: &str| {
        std::env::var_os("PATH")
            .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(exe).is_file()))
    };
    let mut shells = vec![Choice {
        label: "Automatic".into(),
        changes: vec![
            change("shell", "program", None),
            change("shell", "args", None),
        ],
    }];
    for (label, exe) in [
        ("PowerShell 7", "pwsh.exe"),
        ("Windows PowerShell", "powershell.exe"),
        ("Command Prompt", "cmd.exe"),
        ("WSL", "wsl.exe"),
    ] {
        if on_path(exe) {
            shells.push(Choice {
                label: label.into(),
                changes: set(exe, &[]),
            });
        }
    }
    let program_files = std::env::var_os("ProgramFiles")
        .map_or_else(|| PathBuf::from(r"C:\Program Files"), PathBuf::from);
    let git_bash = program_files.join(r"Git\bin\bash.exe");
    if git_bash.is_file() {
        shells.push(Choice {
            label: "Git Bash".into(),
            changes: set(&git_bash.to_string_lossy(), &["--login", "-i"]),
        });
    }
    shells
}

/// Automatic first, then the regular weight of each monospace family in the
/// system and per-user font folders.
fn detect_fonts() -> Vec<Choice> {
    let known = [
        ("cascadiamono", "Cascadia Mono"),
        ("cascadiacode", "Cascadia Code"),
        ("consola", "Consolas"),
        ("lucon", "Lucida Console"),
        ("cour", "Courier New"),
    ];
    let mut dirs = vec![PathBuf::from(r"C:\Windows\Fonts")];
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join(r"Microsoft\Windows\Fonts"));
    }
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
            if !matches!(ext.as_deref(), Some("ttf" | "otf")) {
                continue;
            }
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if let Some(name) = font_name(&stem, &known) {
                found.push((name, path));
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found.dedup_by(|a, b| a.0 == b.0);
    std::iter::once(Choice {
        label: "Automatic".into(),
        changes: vec![change("font", "path", None)],
    })
    .chain(found.into_iter().map(|(name, path)| Choice {
        label: name,
        changes: vec![change(
            "font",
            "path",
            Some(Value::Str(path.to_string_lossy().into_owned())),
        )],
    }))
    .collect()
}

/// The display name of a monospace font file, or `None` for other fonts and
/// for bold, italic, and other non-regular weights.
fn font_name(stem: &str, known: &[(&str, &str)]) -> Option<String> {
    let lower = stem.to_lowercase();
    if let Some((_, name)) = known.iter().find(|(file, _)| *file == lower) {
        return Some((*name).to_owned());
    }
    let (family, weight) = stem.split_once('-').unwrap_or((stem, "Regular"));
    let family_lower = family.to_lowercase();
    if !family_lower.contains("mono") || weight != "Regular" || family_lower.contains("propo") {
        return None;
    }
    // "FiraCodeNerdFontMono" -> "Fira Code Nerd Font Mono".
    let mut name = String::new();
    for (i, c) in family.chars().enumerate() {
        if i > 0 && c.is_uppercase() {
            name.push(' ');
        }
        name.push(c);
    }
    Some(name)
}

/// The key part of a hotkey, in config.toml's spelling.
fn hotkey_key(key: &Key) -> Option<String> {
    match key {
        Key::Named(NamedKey::Space) => Some("Space".into()),
        Key::Named(NamedKey::Enter) => Some("Enter".into()),
        Key::Named(NamedKey::Tab) => Some("Tab".into()),
        Key::Named(named) => {
            let text = format!("{named:?}");
            let n: u32 = text.strip_prefix('F')?.parse().ok()?;
            (1..=24).contains(&n).then(|| format!("F{n}"))
        }
        Key::Character(c) => {
            let ch = c.chars().next()?;
            match ch {
                '`' => Some("`".into()),
                c if c.is_ascii_alphanumeric() => Some(c.to_ascii_uppercase().to_string()),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::SmolStr;

    fn settings() -> Settings {
        let config = Config::default();
        let mut s = Settings {
            section: 0,
            selected: 0,
            recording: false,
            rows: Vec::new(),
            shells: vec![
                Choice {
                    label: "Automatic".into(),
                    changes: vec![change("shell", "program", None)],
                },
                Choice {
                    label: "Command Prompt".into(),
                    changes: vec![change(
                        "shell",
                        "program",
                        Some(Value::Str("cmd.exe".into())),
                    )],
                },
            ],
            fonts: Vec::new(),
            error: None,
        };
        s.rebuild(&config);
        s
    }

    fn named(key: NamedKey) -> Key {
        Key::Named(key)
    }

    #[test]
    fn toggles_save_the_flipped_value() {
        let mut s = settings();
        let config = Config::default();
        s.key(
            &named(NamedKey::ArrowDown),
            ModifiersState::empty(),
            &config,
        );
        assert_eq!(
            s.key(&named(NamedKey::Enter), ModifiersState::empty(), &config),
            Outcome::Save(vec![change(
                "shell",
                "integration",
                Some(Value::Bool(false))
            )])
        );
    }

    #[test]
    fn choices_cycle_both_ways() {
        let mut s = settings();
        let config = Config::default();
        let right = s.key(
            &named(NamedKey::ArrowRight),
            ModifiersState::empty(),
            &config,
        );
        assert_eq!(
            right,
            Outcome::Save(vec![change(
                "shell",
                "program",
                Some(Value::Str("cmd.exe".into()))
            )])
        );
        let left = s.key(
            &named(NamedKey::ArrowLeft),
            ModifiersState::empty(),
            &config,
        );
        assert_eq!(left, right, "two options: back wraps to the other one");
    }

    #[test]
    fn numbers_step_and_stay_in_range() {
        let mut s = settings();
        let config = Config::default();
        s.key(&named(NamedKey::Tab), ModifiersState::empty(), &config); // Appearance
        s.key(
            &named(NamedKey::ArrowDown),
            ModifiersState::empty(),
            &config,
        ); // Font size
        assert_eq!(
            s.key(
                &named(NamedKey::ArrowRight),
                ModifiersState::empty(),
                &config
            ),
            Outcome::Save(vec![change("font", "size", Some(Value::Float(11.5)))])
        );
        s.key(
            &named(NamedKey::ArrowDown),
            ModifiersState::empty(),
            &config,
        ); // Padding
        assert_eq!(
            s.key(
                &named(NamedKey::ArrowLeft),
                ModifiersState::empty(),
                &config
            ),
            Outcome::Save(vec![change("window", "padding", Some(Value::Int(6)))])
        );
    }

    #[test]
    fn records_a_hotkey_with_modifiers() {
        let mut s = settings();
        let config = Config::default();
        s.key(&named(NamedKey::Tab), ModifiersState::empty(), &config);
        s.key(&named(NamedKey::Tab), ModifiersState::empty(), &config); // Launcher
        assert_eq!(
            s.key(&named(NamedKey::Enter), ModifiersState::empty(), &config),
            Outcome::Redraw
        );
        // A bare letter is refused and recording continues.
        s.key(
            &Key::Character(SmolStr::new("k")),
            ModifiersState::empty(),
            &config,
        );
        assert!(s.recording && s.error.is_some());
        let mods = ModifiersState::CONTROL | ModifiersState::ALT;
        assert_eq!(
            s.key(&Key::Character(SmolStr::new("k")), mods, &config),
            Outcome::Save(vec![change(
                "launcher",
                "hotkey",
                Some(Value::Str("Ctrl+Alt+K".into()))
            )])
        );
        assert!(!s.recording);
    }

    #[test]
    fn a_custom_search_url_stays_selectable() {
        let mut config = Config::default();
        config.launcher.search_url = "https://kagi.com/search?q={query}".into();
        let rows = rows(Section::Launcher, &config, &[], &[]);
        let Control::Choice { options, index } = &rows[1].control else {
            panic!("web search is a choice");
        };
        assert_eq!(options[*index].label, "Custom");
    }

    #[test]
    fn font_names_skip_other_weights() {
        let known = [("consola", "Consolas")];
        assert_eq!(font_name("consola", &known).as_deref(), Some("Consolas"));
        assert_eq!(font_name("consolab", &known), None);
        assert_eq!(
            font_name("FiraCodeNerdFontMono-Regular", &known).as_deref(),
            Some("Fira Code Nerd Font Mono")
        );
        assert_eq!(font_name("FiraCodeNerdFontMono-Bold", &known), None);
        assert_eq!(font_name("FiraCodeNerdFontPropo-Regular", &known), None);
        assert_eq!(font_name("Arial", &known), None);
    }

    #[test]
    fn escape_closes_and_cancels_recording_first() {
        let mut s = settings();
        let config = Config::default();
        s.recording = true;
        assert_eq!(
            s.key(&named(NamedKey::Escape), ModifiersState::empty(), &config),
            Outcome::Redraw
        );
        assert_eq!(
            s.key(&named(NamedKey::Escape), ModifiersState::empty(), &config),
            Outcome::Close
        );
    }
}
