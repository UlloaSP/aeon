//! The settings panel: sections on the left, rows on the right, each row a
//! setting saved straight to config.toml (comments kept, see
//! `aeon_config::edit`). It takes the place of the terminal while open.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use aeon_config::edit::{Change, Value};
use aeon_config::{Channel, Config, CursorStyle, themes};
use aeon_launcher::ResidentCommand;
use aeon_launcher::update::Status;
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::keys::Combo;
use crate::render::{Frame, blend, fill_rect};
use crate::sys::Face;
use crate::ui::{self, Palette, REGULAR, SEMIBOLD, Ui, icon};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    General,
    Appearance,
    Keyboard,
    Launcher,
    About,
}

const SECTIONS: [(Section, &str); 5] = [
    (Section::General, "General"),
    (Section::Appearance, "Appearance"),
    (Section::Keyboard, "Keyboard"),
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
    /// A key combination, recorded by pressing it.
    Hotkey {
        value: String,
        table: &'static str,
        key: &'static str,
    },
    Info(String),
    Button {
        label: &'static str,
        action: Action,
    },
}

#[derive(Debug, Clone, Copy)]
enum Action {
    OpenConfigFile,
    Update(ResidentCommand),
}

#[derive(Debug, Clone)]
struct Row {
    title: &'static str,
    description: Cow<'static, str>,
    control: Control,
}

/// What the app must do after an input.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    None,
    Redraw,
    Save(Vec<Change>),
    OpenConfigFile,
    /// Ask the resident to check for, download, or install an update.
    Update(ResidentCommand),
    Close,
}

pub struct Settings {
    section: usize,
    selected: usize,
    recording: bool,
    rows: Vec<Row>,
    shells: Vec<Choice>,
    fonts: Vec<Choice>,
    /// What the resident reports about updates, for About.
    pub update: Status,
    /// Shown under the rows: why the last save failed.
    pub error: Option<String>,
}

impl Settings {
    pub fn new(config: &Config, update: Status) -> Self {
        let mut settings = Self {
            section: 0,
            selected: 0,
            recording: false,
            rows: Vec::new(),
            shells: detect_shells(),
            fonts: detect_fonts(),
            update,
            error: None,
        };
        settings.rebuild(config);
        settings
    }

    /// Rows for the current section, from what the config says now.
    pub fn rebuild(&mut self, config: &Config) {
        self.rows = rows(
            SECTIONS[self.section].0,
            config,
            &self.shells,
            &self.fonts,
            &self.update,
        );
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
            return self.record(key, mods, config);
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
            Control::Button {
                action: Action::OpenConfigFile,
                ..
            } => Outcome::OpenConfigFile,
            Control::Button {
                action: Action::Update(command),
                ..
            } => Outcome::Update(*command),
            _ => Outcome::None,
        }
    }

    pub fn is_recording(&self) -> bool {
        self.recording
    }

    /// While recording, the next key with at least one modifier becomes the
    /// selected row's shortcut. Escape cancels.
    fn record(&mut self, key: &Key, mods: ModifiersState, config: &Config) -> Outcome {
        if let Key::Named(NamedKey::Escape) = key {
            self.recording = false;
            return Outcome::Redraw;
        }
        let Some(combo) = Combo::from_event(key, mods) else {
            return Outcome::None; // a modifier alone: keep waiting
        };
        if !combo.has_modifier() {
            self.error = Some("A shortcut needs Ctrl, Alt, Shift, or Win.".into());
            return Outcome::Redraw;
        }
        let Some(Row {
            control: Control::Hotkey {
                table, key: name, ..
            },
            ..
        }) = self.rows.get(self.selected)
        else {
            self.recording = false;
            return Outcome::Redraw;
        };
        let (table, name) = (*table, *name);
        let text = combo.to_string();
        // The launcher's hotkey is registered with Windows, which takes
        // fewer keys than a window does.
        if table == "launcher" && text.parse::<aeon_launcher::Hotkey>().is_err() {
            self.error = Some(format!(
                "{text} can't be a global hotkey. Try a letter, digit, F-key, or Space."
            ));
            return Outcome::Redraw;
        }
        if let Some(other) = shortcut_names(config)
            .into_iter()
            .find(|(key, value)| *key != name && value.parse::<Combo>().is_ok_and(|c| c == combo))
        {
            self.error = Some(format!(
                "{text} is already used for {}.",
                other.0.replace('_', " ")
            ));
            return Outcome::Redraw;
        }
        self.recording = false;
        self.error = None;
        Outcome::Save(vec![change(table, name, Some(Value::Str(text)))])
    }

    /// Whether a click at this point would do something: a section or a row.
    pub fn is_clickable(&self, x: f64, y: f64, geo: &Geometry) -> bool {
        if x < f64::from(geo.left) {
            return false;
        }
        if x < f64::from(geo.left + geo.nav) {
            return geo.nav_item_at(y).is_some_and(|i| i < SECTIONS.len());
        }
        geo.row_at(y, self.rows.len()).is_some()
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

    pub fn draw(&self, frame: &mut Frame<'_>, ui: &mut Ui, pal: &Palette, geo: &Geometry) {
        let body = ui.font(Face::Ui, 14.0, REGULAR);
        let strong = ui.font(Face::Ui, 14.0, SEMIBOLD);
        let small = ui.font(Face::Ui, 12.0, REGULAR);
        let width = frame.width;
        let content = geo.left + geo.nav;
        fill_rect(
            frame,
            geo.left,
            0,
            width.saturating_sub(geo.left),
            frame.height,
            pal.canvas,
        );
        fill_rect(
            frame,
            content.saturating_sub(1),
            0,
            1,
            frame.height,
            pal.border,
        );

        for (i, (_, name)) in SECTIONS.iter().enumerate() {
            let top = geo.nav_top + i as u32 * geo.nav_row;
            let x = geo.left + geo.inset;
            let w = geo.nav.saturating_sub(2 * geo.inset);
            let on = i == self.section;
            if on {
                ui::round_rect(frame, x, top, w, geo.nav_row, ui.px(6.0), pal.highlight);
            }
            let mid = top + geo.nav_row / 2;
            let color = if on { pal.fg } else { pal.muted };
            ui.text_mid(
                frame,
                x + ui.px(10.0),
                mid,
                name,
                if on { strong } else { body },
                color,
                w,
            );
        }

        // Header: the section, and where changes go.
        let hx = content + geo.pad + ui.px(8.0);
        let hmid = geo.header / 2;
        fill_rect(
            frame,
            content,
            geo.header.saturating_sub(1),
            width.saturating_sub(content),
            1,
            pal.border,
        );
        let title = SECTIONS[self.section].1;
        let tw = ui.text_mid(frame, hx, hmid, title, strong, pal.fg, ui.px(240.0));
        ui.text_mid(
            frame,
            hx + tw + ui.px(12.0),
            hmid,
            "Saved to config.toml, comments kept",
            small,
            pal.label,
            width.saturating_sub(hx + tw + ui.px(28.0)),
        );

        let rows = self.rows.len() as u32;
        let card_h = rows * geo.row;
        if rows > 0 {
            let fill = blend(pal.canvas, pal.raised, 120);
            ui::card(
                frame,
                geo.card_x,
                geo.card_y,
                geo.card_w,
                card_h,
                ui.px(12.0),
                fill,
                pal.border,
            );
        }
        let text_x = geo.card_x + geo.pad;
        let text_w = geo.control_x.saturating_sub(text_x + geo.pad);
        for (i, row) in self.rows.iter().enumerate() {
            let top = geo.card_y + i as u32 * geo.row;
            if top + geo.row > frame.height {
                break;
            }
            if i > 0 {
                fill_rect(
                    frame,
                    geo.card_x + 1,
                    top,
                    geo.card_w.saturating_sub(2),
                    1,
                    pal.border,
                );
            }
            let selected = i == self.selected;
            if selected {
                let inset = ui.px(4.0);
                ui::round_rect(
                    frame,
                    geo.card_x + inset,
                    top + inset,
                    geo.card_w.saturating_sub(2 * inset),
                    geo.row.saturating_sub(2 * inset),
                    ui.px(8.0),
                    blend(fill_of(pal), pal.fg, 14),
                );
            }
            let mid = top + geo.row / 2;
            ui.text_mid(
                frame,
                text_x,
                mid - ui.px(9.0),
                row.title,
                body,
                pal.fg,
                text_w,
            );
            ui.text_mid(
                frame,
                text_x,
                mid + ui.px(10.0),
                &row.description,
                small,
                pal.label,
                text_w,
            );
            self.draw_control(frame, ui, pal, geo, &row.control, mid, selected);
        }

        let hint = if self.recording {
            "Press the new shortcut \u{b7} Esc cancels"
        } else {
            "\u{2191}\u{2193} move \u{b7} \u{2190}\u{2192} change \u{b7} Enter toggles \u{b7} Tab next section \u{b7} Esc closes"
        };
        let below = geo.card_y + card_h + ui.px(16.0);
        let line = ui.line_height(small);
        let mut y = below;
        if let Some(error) = &self.error {
            ui.text(frame, geo.card_x, y, error, small, pal.red, geo.card_w);
            y += line + ui.px(6.0);
        }
        ui.text(frame, geo.card_x, y, hint, small, pal.label, geo.card_w);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_control(
        &self,
        frame: &mut Frame<'_>,
        ui: &mut Ui,
        pal: &Palette,
        geo: &Geometry,
        control: &Control,
        mid: u32,
        selected: bool,
    ) {
        let body = ui.font(Face::Ui, 14.0, REGULAR);
        let small = ui.font(Face::Ui, 12.0, REGULAR);
        let right = geo.control_x + geo.control_w;
        match control {
            Control::Toggle { on, .. } => {
                let (w, h) = (ui.px(32.0), ui.px(18.0));
                let x = right - w;
                let track = if *on {
                    pal.accent
                } else {
                    blend(pal.canvas, pal.fg, 34)
                };
                ui::round_rect(frame, x, mid - h / 2, w, h, h / 2, track);
                let knob = ui.px(14.0);
                let inset = (h - knob) / 2;
                let kx = if *on {
                    x + w - inset - knob / 2
                } else {
                    x + inset + knob / 2
                };
                ui::dot(frame, kx, mid, knob, pal.canvas);
            }
            Control::Choice { options, index } => {
                let label = options.get(*index).map_or("", |o| o.label.as_str());
                self.stepper(frame, ui, pal, geo, label, mid, selected);
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
                self.stepper(
                    frame,
                    ui,
                    pal,
                    geo,
                    &format!("{number} {unit}"),
                    mid,
                    selected,
                );
            }
            Control::Hotkey { value, .. } => {
                let recording = self.recording && selected;
                let text = if recording {
                    "Press keys\u{2026}"
                } else {
                    value.as_str()
                };
                let tw = ui.measure(text, small);
                let (w, h) = (tw + ui.px(16.0), ui.px(24.0));
                let x = right - w;
                let (bg, fg) = if recording {
                    (blend(pal.canvas, pal.accent, 36), pal.accent)
                } else {
                    (blend(pal.canvas, pal.fg, 20), pal.fg)
                };
                ui::round_rect(frame, x, mid - h / 2, w, h, ui.px(5.0), bg);
                ui.text_mid(frame, x + ui.px(8.0), mid, text, small, fg, w);
            }
            Control::Info(text) => {
                ui.text_right(frame, right, mid, text, body, pal.muted);
            }
            Control::Button { label, .. } => {
                let tw = ui.measure(label, body);
                let (w, h) = (tw + ui.px(24.0), ui.px(30.0));
                let x = right - w;
                let border = if selected {
                    pal.accent
                } else {
                    blend(pal.canvas, pal.fg, 31)
                };
                ui::card(
                    frame,
                    x,
                    mid - h / 2,
                    w,
                    h,
                    ui.px(8.0),
                    blend(pal.canvas, pal.fg, 9),
                    border,
                );
                ui.text_mid(frame, x + ui.px(12.0), mid, label, body, pal.fg, w);
            }
        }
    }

    /// A value in a select-like box with ‹ › at its ends; clicking the left
    /// half steps back, the right half forward.
    #[allow(clippy::too_many_arguments)]
    fn stepper(
        &self,
        frame: &mut Frame<'_>,
        ui: &mut Ui,
        pal: &Palette,
        geo: &Geometry,
        label: &str,
        mid: u32,
        selected: bool,
    ) {
        let body = ui.font(Face::Ui, 14.0, REGULAR);
        let h = ui.px(32.0);
        let x = geo.control_x;
        let w = geo.control_w;
        let border = if selected {
            pal.accent
        } else {
            blend(pal.canvas, pal.fg, 31)
        };
        ui::card(
            frame,
            x,
            mid - h / 2,
            w,
            h,
            ui.px(8.0),
            blend(pal.canvas, pal.fg, 9),
            border,
        );
        let arrows = if selected { pal.accent } else { pal.label };
        ui.icon(
            frame,
            x + ui.px(14.0),
            mid,
            icon::CHEVRON_LEFT,
            10.0,
            arrows,
        );
        ui.icon(
            frame,
            x + w - ui.px(14.0),
            mid,
            icon::CHEVRON_RIGHT,
            10.0,
            arrows,
        );
        let room = w.saturating_sub(ui.px(56.0));
        let tw = ui.measure(label, body).min(room);
        ui.text_mid(frame, x + (w - tw) / 2, mid, label, body, pal.fg, room);
    }
}

fn fill_of(pal: &Palette) -> u32 {
    blend(pal.canvas, pal.raised, 120)
}

/// The panel's measurements, shared by drawing and click handling.
pub struct Geometry {
    /// First pixel column of the panel (right of the sidebar).
    left: u32,
    nav: u32,
    inset: u32,
    pad: u32,
    nav_top: u32,
    nav_row: u32,
    header: u32,
    card_x: u32,
    card_y: u32,
    card_w: u32,
    row: u32,
    control_x: u32,
    control_w: u32,
}

impl Geometry {
    pub fn new(left: u32, width: u32, scale: f64) -> Self {
        let px = |logical: f64| (logical * scale).round() as u32;
        let nav = px(240.0).min(width.saturating_sub(left) / 3);
        let content = left + nav;
        let pad = px(16.0);
        let card_x = content + px(24.0);
        let card_w = width.saturating_sub(content + px(48.0)).min(px(896.0));
        let control_w = px(220.0).min(card_w / 2);
        let header = px(44.0);
        Self {
            left,
            nav,
            inset: px(8.0),
            pad,
            nav_top: px(12.0),
            nav_row: px(32.0),
            header,
            card_x,
            card_y: header + px(24.0),
            card_w,
            row: px(60.0),
            control_x: (card_x + card_w).saturating_sub(pad + control_w),
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
        if y < f64::from(self.card_y) {
            return None;
        }
        let i = ((y - f64::from(self.card_y)) / f64::from(self.row)) as usize;
        (i < rows).then_some(i)
    }
}

fn change(table: &'static str, key: &'static str, value: Option<Value>) -> Change {
    Change { table, key, value }
}

fn rows(
    section: Section,
    config: &Config,
    shells: &[Choice],
    fonts: &[Choice],
    update: &Status,
) -> Vec<Row> {
    match section {
        Section::General => vec![
            Row {
                title: "Shell",
                description: "What new sessions run.".into(),
                control: Control::Choice {
                    index: current_shell(config, shells),
                    options: shells.to_vec(),
                },
            },
            Row {
                title: "Shell integration",
                description: "Aeon's prompt and history. Off loads your PowerShell profile.".into(),
                control: Control::Toggle {
                    on: config.shell.integration,
                    table: "shell",
                    key: "integration",
                },
            },
            Row {
                title: "Session header",
                description: "Project, branch, changes, and servers above the terminal.".into(),
                control: Control::Toggle {
                    on: config.bar.enabled,
                    table: "bar",
                    key: "enabled",
                },
            },
            Row {
                title: "Restore sessions",
                description: "Reopen the last window's sessions and panes.".into(),
                control: Control::Toggle {
                    on: config.window.restore,
                    table: "window",
                    key: "restore",
                },
            },
            Row {
                title: "Start with Windows",
                description: "Keep the launcher ready from sign-in.".into(),
                control: Control::Toggle {
                    on: config.launcher.start_with_windows,
                    table: "launcher",
                    key: "start_with_windows",
                },
            },
        ],
        Section::Appearance => vec![
            Row {
                title: "Theme",
                description: "Colors for the terminal, sidebar, and launcher.".into(),
                control: Control::Choice {
                    index: themes::PRESETS
                        .iter()
                        .position(|p| p.id == config.colors.theme)
                        .unwrap_or(0),
                    options: themes::PRESETS
                        .iter()
                        .map(|p| Choice {
                            label: p.name.to_owned(),
                            changes: vec![change(
                                "colors",
                                "theme",
                                Some(Value::Str(p.id.to_owned())),
                            )],
                        })
                        .collect(),
                },
            },
            Row {
                title: "Cursor",
                description: "How the text cursor is drawn.".into(),
                control: Control::Choice {
                    index: match config.cursor.style {
                        CursorStyle::Block => 0,
                        CursorStyle::Bar => 1,
                        CursorStyle::Underline => 2,
                    },
                    options: ["Block", "Bar", "Underline"]
                        .iter()
                        .map(|label| Choice {
                            label: (*label).to_owned(),
                            changes: vec![change(
                                "cursor",
                                "style",
                                Some(Value::Str(label.to_lowercase())),
                            )],
                        })
                        .collect(),
                },
            },
            Row {
                title: "Font",
                description: "Monospace fonts installed on this PC.".into(),
                control: Control::Choice {
                    index: current_font(config, fonts),
                    options: fonts.to_vec(),
                },
            },
            Row {
                title: "Font size",
                description: "Text size at 100% display scale.".into(),
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
                description: "Space around the terminal text.".into(),
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
        Section::Keyboard => {
            let key_row = |title: &'static str, key: &'static str, value: &str| Row {
                title,
                description: "Enter, then press the new shortcut.".into(),
                control: Control::Hotkey {
                    value: value.to_owned(),
                    table: "keys",
                    key,
                },
            };
            let keys = &config.keys;
            vec![
                key_row("New session", "new_session", &keys.new_session),
                key_row("Close session", "close_session", &keys.close_session),
                key_row("Next session", "next_session", &keys.next_session),
                key_row(
                    "Previous session",
                    "previous_session",
                    &keys.previous_session,
                ),
                key_row(
                    "Show or hide the sidebar",
                    "toggle_sidebar",
                    &keys.toggle_sidebar,
                ),
                key_row("Settings", "settings", &keys.settings),
                key_row("Find", "find", &keys.find),
                key_row("Copy", "copy", &keys.copy),
                key_row("Paste", "paste", &keys.paste),
                key_row("Project panel", "project_panel", &keys.project_panel),
                key_row("Split right", "split_right", &keys.split_right),
                key_row("Split down", "split_down", &keys.split_down),
                key_row("Zoom the pane", "zoom_pane", &keys.zoom_pane),
                key_row("Rename session", "rename_session", &keys.rename_session),
                Row {
                    title: "Move between panes",
                    description: "Fixed: in a split session.".into(),
                    control: Control::Info("Alt+Arrows".to_owned()),
                },
                Row {
                    title: "Go to a session",
                    description: "Fixed: by its number in the sidebar.".into(),
                    control: Control::Info("Ctrl+1 \u{2026} Ctrl+9".to_owned()),
                },
            ]
        }
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
                    description: "Opens the launcher from anywhere. Enter to record.".into(),
                    control: Control::Hotkey {
                        value: config.launcher.hotkey.clone(),
                        table: "launcher",
                        key: "hotkey",
                    },
                },
                Row {
                    title: "Web search",
                    description: "Where queries that are not a command or app go.".into(),
                    control: Control::Choice { options, index },
                },
            ]
        }
        Section::About => vec![
            Row {
                title: "Version",
                description: "The Aeon build you are running.".into(),
                control: Control::Info(crate::VERSION.to_owned()),
            },
            update_row(update),
            Row {
                title: "Update channel",
                description: "Nightly: daily builds of what comes next.".into(),
                control: Control::Choice {
                    index: usize::from(config.updates.channel == Channel::Nightly),
                    options: vec![
                        Choice {
                            label: "Stable".into(),
                            changes: vec![change("updates", "channel", None)],
                        },
                        Choice {
                            label: "Nightly".into(),
                            changes: vec![change(
                                "updates",
                                "channel",
                                Some(Value::Str("nightly".into())),
                            )],
                        },
                    ],
                },
            },
            Row {
                title: "Check automatically",
                description: "At sign-in and every 4 hours. Nothing installs by itself.".into(),
                control: Control::Toggle {
                    on: config.updates.check,
                    table: "updates",
                    key: "check",
                },
            },
            Row {
                title: "Configuration file",
                description: "Every setting, including ones not shown here.".into(),
                control: Control::Button {
                    label: "Open",
                    action: Action::OpenConfigFile,
                },
            },
        ],
    }
}

/// What the user can do about updates right now.
fn update_row(status: &Status) -> Row {
    let row = |description: String, control| Row {
        title: "Updates",
        description: description.into(),
        control,
    };
    let button = |label, command| Control::Button {
        label,
        action: Action::Update(command),
    };
    let check = || button("Check now", ResidentCommand::CheckUpdates);
    match status {
        Status::Idle => row(
            "Aeon looks for new versions in the background.".into(),
            check(),
        ),
        Status::UpToDate => row("Aeon is up to date.".into(), check()),
        Status::Error(message) => row(message.clone(), check()),
        Status::Checking => row(
            "Checking for updates\u{2026}".into(),
            Control::Info(String::new()),
        ),
        Status::Available(release) => row(
            format!("Aeon {} is available.", release.version),
            button("Download", ResidentCommand::DownloadUpdate),
        ),
        Status::Downloading { release, percent } => row(
            format!("Downloading Aeon {}.", release.version),
            Control::Info(format!("{percent}%")),
        ),
        Status::Ready { release, .. } => row(
            format!(
                "Aeon {} is ready. Aeon closes to install it.",
                release.version
            ),
            button("Restart to update", ResidentCommand::InstallUpdate),
        ),
    }
}

/// Every configurable shortcut by its config key, to catch duplicates.
fn shortcut_names(config: &Config) -> Vec<(&'static str, String)> {
    let k = &config.keys;
    vec![
        ("new_session", k.new_session.clone()),
        ("close_session", k.close_session.clone()),
        ("next_session", k.next_session.clone()),
        ("previous_session", k.previous_session.clone()),
        ("toggle_sidebar", k.toggle_sidebar.clone()),
        ("settings", k.settings.clone()),
        ("find", k.find.clone()),
        ("copy", k.copy.clone()),
        ("paste", k.paste.clone()),
        ("project_panel", k.project_panel.clone()),
        ("split_right", k.split_right.clone()),
        ("split_down", k.split_down.clone()),
        ("zoom_pane", k.zoom_pane.clone()),
        ("rename_session", k.rename_session.clone()),
        ("hotkey", config.launcher.hotkey.clone()),
    ]
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
            update: Status::Idle,
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
        // Past Theme and Cursor; the next Down lands on Font size.
        for _ in 0..2 {
            s.key(
                &named(NamedKey::ArrowDown),
                ModifiersState::empty(),
                &config,
            );
        }
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
        for _ in 0..3 {
            s.key(&named(NamedKey::Tab), ModifiersState::empty(), &config);
        } // Launcher
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
    fn shortcuts_record_reject_duplicates_and_launcher_limits() {
        let mut s = settings();
        let config = Config::default();
        s.key(&named(NamedKey::Tab), ModifiersState::empty(), &config);
        s.key(&named(NamedKey::Tab), ModifiersState::empty(), &config); // Keyboard
        let ctrl_shift = ModifiersState::CONTROL | ModifiersState::SHIFT;
        // New session -> Ctrl+Shift+W is taken by Close session.
        s.key(&named(NamedKey::Enter), ModifiersState::empty(), &config);
        s.key(&Key::Character(SmolStr::new("W")), ctrl_shift, &config);
        assert!(
            s.recording
                && s.error
                    .as_deref()
                    .is_some_and(|e| e.contains("close session"))
        );
        assert_eq!(
            s.key(&Key::Character(SmolStr::new("N")), ctrl_shift, &config),
            Outcome::Save(vec![change(
                "keys",
                "new_session",
                Some(Value::Str("Ctrl+Shift+N".into()))
            )])
        );
        // The launcher's hotkey must be one Windows can register.
        for _ in 0..1 {
            s.key(&named(NamedKey::Tab), ModifiersState::empty(), &config);
        } // Launcher
        s.key(&named(NamedKey::Enter), ModifiersState::empty(), &config);
        s.key(
            &Key::Character(SmolStr::new(",")),
            ModifiersState::CONTROL,
            &config,
        );
        assert!(
            s.error
                .as_deref()
                .is_some_and(|e| e.contains("global hotkey"))
        );
    }

    #[test]
    fn a_custom_search_url_stays_selectable() {
        let mut config = Config::default();
        config.launcher.search_url = "https://kagi.com/search?q={query}".into();
        let rows = rows(Section::Launcher, &config, &[], &[], &Status::Idle);
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
