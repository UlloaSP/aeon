//! The sidebar and the session header, laid out like T3 Code: projects own
//! their sessions, and a session's state is a small dot beside its name.

use std::path::{Path, PathBuf};

use aeon_context::Context;

use crate::render::{Frame, blend, fill_rect};
use crate::sys::Face;
use crate::ui::{self, Palette, REGULAR, SEMIBOLD, Ui, icon};

/// Sessions that run in the same repository, or the same folder outside one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    /// `None` for Home: the home folder and sessions with no folder yet.
    pub path: Option<PathBuf>,
    /// Indexes into the session list, in session order.
    pub sessions: Vec<usize>,
}

/// Groups sessions into projects, ordered by each project's first session.
pub fn projects(contexts: &[&Context], home: Option<&Path>) -> Vec<Project> {
    let mut projects: Vec<Project> = Vec::new();
    for (index, context) in contexts.iter().enumerate() {
        let path = context
            .root
            .as_deref()
            .or(context.cwd.as_deref())
            .filter(|p| Some(*p) != home)
            .map(Path::to_path_buf);
        match projects.iter_mut().find(|p| p.path == path) {
            Some(project) => project.sessions.push(index),
            None => projects.push(Project {
                name: path
                    .as_deref()
                    .and_then(Path::file_name)
                    .map_or_else(|| "Home".to_owned(), |n| n.to_string_lossy().into_owned()),
                path,
                sessions: vec![index],
            }),
        }
    }
    projects
}

/// Sessions in the order the sidebar shows them; Ctrl+Tab and Ctrl+1-9
/// follow it.
pub fn display_order(projects: &[Project]) -> Vec<usize> {
    projects
        .iter()
        .flat_map(|p| p.sessions.iter().copied())
        .collect()
}

/// Two letters for a project's tile: the first letters of its first and last
/// words (`web-app` is WA), or its first two letters.
pub fn monogram(name: &str) -> String {
    let words: Vec<&str> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let letters: String = match words.as_slice() {
        [] => "?".to_owned(),
        [only] => only.chars().take(2).collect(),
        [first, .., last] => first.chars().take(1).chain(last.chars().take(1)).collect(),
    };
    letters.to_uppercase()
}

/// A project's colour, stable for its name. Home stays neutral.
pub fn project_color(project: &Project, pal: &Palette) -> u32 {
    if project.path.is_none() {
        return pal.muted;
    }
    let hues = [
        pal.yellow,
        pal.blue,
        pal.green,
        pal.magenta,
        pal.cyan,
        pal.red,
    ];
    let hash = project.name.bytes().fold(2166136261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16777619)
    });
    hues[hash as usize % hues.len()]
}

/// What a session is called: its shell at the project's top, otherwise the
/// folder it is in, relative to the project.
pub fn session_title(
    context: &Context,
    shell: &str,
    project: &Project,
    home: Option<&Path>,
) -> String {
    let Some(cwd) = context.cwd.as_deref() else {
        return shell.to_owned();
    };
    let base = project.path.as_deref().or(home);
    match base.and_then(|b| cwd.strip_prefix(b).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => shell.to_owned(),
        Some(rest) => rest.display().to_string(),
        None => cwd.file_name().map_or_else(
            || cwd.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

impl Rect {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= f64::from(self.x)
            && x < f64::from(self.x + self.w)
            && y >= f64::from(self.y)
            && y < f64::from(self.y + self.h)
    }

    fn mid(&self) -> u32 {
        self.y + self.h / 2
    }
}

/// Something in the sidebar a click acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    NewSession,
    Session(usize),
    /// The bell on a session row: mutes or unmutes its notifications.
    Mute(usize),
    Settings,
    Update,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    NewSession,
    Project(usize),
    Session(usize),
}

/// The update notice at the foot of the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pill {
    Available,
    Downloading(u8),
    Ready,
    Failed,
    Checking,
}

impl Pill {
    fn label(&self) -> String {
        match self {
            Pill::Available => "Update".into(),
            Pill::Downloading(percent) => format!("{percent}%"),
            Pill::Ready => "Restart to update".into(),
            Pill::Failed => "Retry update".into(),
            Pill::Checking => "Checking\u{2026}".into(),
        }
    }
}

/// Where everything in the sidebar is, shared by drawing and hit-testing so
/// the two never disagree.
pub struct Sidebar {
    pub width: u32,
    rows: Vec<(Rect, Row)>,
    /// Width of the mute button at the right end of a hovered session row.
    mute: u32,
    footer_top: u32,
    settings: Rect,
    update: Option<Rect>,
}

impl Sidebar {
    pub fn new(
        ui: &mut Ui,
        width: u32,
        height: u32,
        projects: &[Project],
        pill: Option<&Pill>,
    ) -> Self {
        let inset = ui.px(8.0);
        let inner = width.saturating_sub(2 * inset);
        let footer_top = height.saturating_sub(ui.px(50.0));
        let mut rows = Vec::new();
        let mut y = ui.px(40.0);
        let mut push = |y: &mut u32, h: u32, row: Row| {
            if *y + h <= footer_top {
                rows.push((
                    Rect {
                        x: inset,
                        y: *y,
                        w: inner,
                        h,
                    },
                    row,
                ));
            }
            *y += h;
        };
        push(&mut y, ui.px(32.0), Row::NewSession);
        y += ui.px(6.0);
        for (i, project) in projects.iter().enumerate() {
            if i > 0 {
                y += ui.px(6.0);
            }
            push(&mut y, ui.px(32.0), Row::Project(i));
            for &session in &project.sessions {
                push(&mut y, ui.px(36.0), Row::Session(session));
            }
        }
        let button = ui.px(32.0);
        let row_y = footer_top + ui.px(9.0);
        let update = pill.map(|pill| {
            let w = pill_width(ui, pill);
            Rect {
                x: width.saturating_sub(inset + w),
                y: row_y,
                w,
                h: button,
            }
        });
        Self {
            width,
            rows,
            mute: ui.px(30.0),
            footer_top,
            settings: Rect {
                x: inset,
                y: row_y,
                w: button,
                h: button,
            },
            update,
        }
    }

    pub fn hit(&self, x: f64, y: f64) -> Option<Hit> {
        if self.settings.contains(x, y) {
            return Some(Hit::Settings);
        }
        if self.update.is_some_and(|r| r.contains(x, y)) {
            return Some(Hit::Update);
        }
        self.rows
            .iter()
            .find(|(rect, _)| rect.contains(x, y))
            .and_then(|(rect, row)| match row {
                Row::NewSession => Some(Hit::NewSession),
                Row::Session(i) if x >= f64::from(rect.x + rect.w - self.mute) => {
                    Some(Hit::Mute(*i))
                }
                Row::Session(i) => Some(Hit::Session(*i)),
                Row::Project(_) => None,
            })
    }
}

fn pill_width(ui: &mut Ui, pill: &Pill) -> u32 {
    let font = ui.font(Face::Ui, 13.0, SEMIBOLD);
    let text = ui.measure(&pill.label(), font);
    let lead = match pill {
        Pill::Downloading(_) => ui.px(44.0 + 8.0),
        Pill::Checking => 0,
        _ => ui.px(14.0 + 6.0),
    };
    ui.px(10.0) + lead + text + ui.px(12.0)
}

/// One session as the sidebar shows it.
pub struct SessionRow {
    pub title: String,
    pub state: RowState,
    /// It finished, failed, or rang while you were elsewhere: drawn bold
    /// until you look at it.
    pub unseen: bool,
    pub muted: bool,
}

/// What a session is doing, as its dot and the text at the row's end show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    Idle,
    Running,
    Server(u16),
    Progress(u8),
    Failed(i32),
    Done,
    Bell,
}

pub struct SidebarState<'a> {
    pub projects: &'a [Project],
    pub sessions: &'a [SessionRow],
    pub active: usize,
    pub settings_open: bool,
    pub hover: Option<Hit>,
    pub pill: Option<&'a Pill>,
    /// The new-session shortcut, as the user configured it.
    pub new_keys: &'a str,
}

pub fn draw_sidebar(
    frame: &mut Frame<'_>,
    ui: &mut Ui,
    pal: &Palette,
    sb: &Sidebar,
    state: &SidebarState<'_>,
) {
    let height = frame.height;
    fill_rect(frame, 0, 0, sb.width, height, pal.canvas);
    fill_rect(frame, sb.width.saturating_sub(1), 0, 1, height, pal.border);

    let body = ui.font(Face::Ui, 14.0, REGULAR);
    let strong = ui.font(Face::Ui, 14.0, SEMIBOLD);
    let small = ui.font(Face::Ui, 12.0, REGULAR);
    let radius = ui.px(6.0);

    // Brand.
    let brand_mid = ui.px(20.0);
    let tile = ui.px(16.0);
    let bx = ui.px(16.0);
    ui::card(
        frame,
        bx,
        brand_mid - tile / 2,
        tile,
        tile,
        ui.px(5.0),
        pal.raised,
        pal.highlight,
    );
    let mark = ui.font(Face::Ui, 11.0, SEMIBOLD);
    let mw = ui.measure("a", mark);
    ui.text_mid(
        frame,
        bx + (tile - mw) / 2,
        brand_mid - ui.px(1.0),
        "a",
        mark,
        pal.accent,
        tile,
    );
    ui.text_mid(
        frame,
        bx + tile + ui.px(8.0),
        brand_mid,
        "Aeon",
        strong,
        pal.fg,
        ui.px(120.0),
    );

    for (rect, row) in &sb.rows {
        let mid = rect.mid();
        match *row {
            Row::NewSession => {
                let hovered = state.hover == Some(Hit::NewSession);
                if hovered {
                    ui::round_rect(frame, rect.x, rect.y, rect.w, rect.h, radius, pal.hover);
                }
                let color = if hovered { pal.fg } else { pal.muted };
                ui.icon(frame, rect.x + ui.px(18.0), mid, icon::ADD, 13.0, color);
                ui.text_mid(
                    frame,
                    rect.x + ui.px(36.0),
                    mid,
                    "New session",
                    body,
                    color,
                    rect.w,
                );
                ui.text_right(
                    frame,
                    rect.x + rect.w - ui.px(10.0),
                    mid,
                    state.new_keys,
                    small,
                    pal.label,
                );
            }
            Row::Project(i) => {
                let project = &state.projects[i];
                let color = project_color(project, pal);
                monogram_tile(
                    frame,
                    ui,
                    pal,
                    rect.x + ui.px(10.0),
                    mid,
                    &project.name,
                    color,
                );
                let name_color = blend(pal.canvas, pal.fg, 230);
                let count = project.sessions.len().to_string();
                let count_x = ui.text_right(
                    frame,
                    rect.x + rect.w - ui.px(10.0),
                    mid,
                    &count,
                    small,
                    pal.label,
                );
                let x = rect.x + ui.px(36.0);
                ui.text_mid(
                    frame,
                    x,
                    mid,
                    &project.name,
                    strong,
                    name_color,
                    count_x.saturating_sub(x + ui.px(8.0)),
                );
            }
            Row::Session(i) => {
                let Some(session) = state.sessions.get(i) else {
                    continue;
                };
                let selected = i == state.active && !state.settings_open;
                let hovered = state.hover == Some(Hit::Session(i));
                if selected {
                    ui::card(
                        frame,
                        rect.x,
                        rect.y,
                        rect.w,
                        rect.h,
                        radius,
                        pal.raised,
                        blend(pal.canvas, pal.fg, 16),
                    );
                } else if hovered {
                    ui::round_rect(frame, rect.x, rect.y, rect.w, rect.h, radius, pal.hover);
                }
                let slot = rect.x + ui.px(43.0);
                let (dot, meta, meta_color) = match session.state {
                    RowState::Idle => (None, String::new(), pal.label),
                    RowState::Running => (Some(pal.blue), "running".to_owned(), pal.label),
                    RowState::Server(port) => (Some(pal.cyan), format!(":{port}"), pal.cyan),
                    RowState::Progress(p) => (Some(pal.blue), format!("{p}%"), pal.label),
                    RowState::Failed(code) => (Some(pal.red), format!("exit {code}"), pal.red),
                    RowState::Done => (Some(pal.green), "done".to_owned(), pal.green),
                    RowState::Bell => (Some(pal.yellow), "bell".to_owned(), pal.yellow),
                };
                match dot {
                    Some(color) => ui::dot(frame, slot, mid, ui.px(9.0), color),
                    None => {
                        let strength = if selected || hovered { 215 } else { 102 };
                        ui.icon(
                            frame,
                            slot,
                            mid,
                            icon::TERMINAL,
                            12.0,
                            blend(pal.canvas, pal.fg, strength),
                        );
                    }
                }
                let right = rect.x + rect.w - ui.px(10.0);
                let meta_x = if hovered {
                    // The row's own action replaces its status while hovered.
                    let on_bell = state.hover == Some(Hit::Mute(i));
                    let glyph = if session.muted {
                        icon::MUTED
                    } else {
                        icon::BELL
                    };
                    let color = if on_bell { pal.fg } else { pal.muted };
                    let cx = right.saturating_sub(ui.px(6.0));
                    ui.icon(frame, cx, mid, glyph, 12.0, color);
                    cx.saturating_sub(ui.px(10.0))
                } else {
                    let mut end = right;
                    if !meta.is_empty() {
                        end = ui.text_right(frame, end, mid, &meta, small, meta_color);
                    }
                    if session.muted {
                        let cx = end.saturating_sub(ui.px(10.0));
                        ui.icon(frame, cx, mid, icon::MUTED, 10.0, pal.label);
                        end = cx.saturating_sub(ui.px(8.0));
                    }
                    end
                };
                let x = rect.x + ui.px(60.0);
                let font = if selected || session.unseen {
                    strong
                } else {
                    body
                };
                ui.text_mid(
                    frame,
                    x,
                    mid,
                    &session.title,
                    font,
                    pal.fg,
                    meta_x.saturating_sub(x + ui.px(8.0)),
                );
            }
        }
    }

    // Footer.
    fill_rect(
        frame,
        0,
        sb.footer_top,
        sb.width.saturating_sub(1),
        1,
        pal.border,
    );
    let s = sb.settings;
    let on = state.settings_open || state.hover == Some(Hit::Settings);
    if on {
        ui::round_rect(
            frame,
            s.x,
            s.y,
            s.w,
            s.h,
            ui.px(8.0),
            if state.settings_open {
                pal.highlight
            } else {
                pal.hover
            },
        );
    }
    ui.icon(
        frame,
        s.x + s.w / 2,
        s.mid(),
        icon::SETTINGS,
        15.0,
        if on { pal.fg } else { pal.muted },
    );

    if let (Some(pill), Some(r)) = (state.pill, sb.update) {
        draw_pill(frame, ui, pal, pill, r, state.hover == Some(Hit::Update));
    }
}

fn draw_pill(
    frame: &mut Frame<'_>,
    ui: &mut Ui,
    pal: &Palette,
    pill: &Pill,
    r: Rect,
    hovered: bool,
) {
    let tint = |c: u32, a: u8| blend(pal.canvas, c, a);
    let (bg, fg) = match pill {
        Pill::Ready => (
            if hovered {
                tint(pal.accent, 225)
            } else {
                pal.accent
            },
            pal.on_accent,
        ),
        Pill::Failed => (tint(pal.red, if hovered { 50 } else { 34 }), pal.red),
        Pill::Checking => (pal.hover, pal.label),
        Pill::Available | Pill::Downloading(_) => {
            (tint(pal.accent, if hovered { 50 } else { 34 }), pal.accent)
        }
    };
    ui::round_rect(frame, r.x, r.y, r.w, r.h, r.h / 2, bg);
    let mid = r.mid();
    let mut x = r.x + ui.px(10.0);
    match pill {
        Pill::Downloading(percent) => {
            let (w, h) = (ui.px(44.0), ui.px(4.0).max(2));
            ui::round_rect(frame, x, mid - h / 2, w, h, h / 2, tint(pal.accent, 64));
            let done = w * u32::from((*percent).min(100)) / 100;
            if done > 0 {
                ui::round_rect(frame, x, mid - h / 2, done, h, h / 2, pal.accent);
            }
            x += w + ui.px(8.0);
        }
        Pill::Checking => {}
        _ => {
            let glyph = if matches!(pill, Pill::Available) {
                icon::DOWNLOAD
            } else {
                icon::REFRESH
            };
            ui.icon(frame, x + ui.px(7.0), mid, glyph, 12.0, fg);
            x += ui.px(20.0);
        }
    }
    let font = ui.font(Face::Ui, 13.0, SEMIBOLD);
    ui.text_mid(frame, x, mid, &pill.label(), font, fg, r.w);
}

/// A project's two-letter tile, 16 logical pixels square.
pub fn monogram_tile(
    frame: &mut Frame<'_>,
    ui: &mut Ui,
    pal: &Palette,
    x: u32,
    mid: u32,
    name: &str,
    color: u32,
) {
    let size = ui.px(16.0);
    ui::round_rect(
        frame,
        x,
        mid - size / 2,
        size,
        size,
        ui.px(4.0),
        blend(pal.canvas, color, 41),
    );
    let font = ui.font(Face::Code, 8.5, SEMIBOLD);
    let letters = monogram(name);
    let w = ui.measure(&letters, font);
    ui.text_mid(
        frame,
        x + size.saturating_sub(w) / 2,
        mid,
        &letters,
        font,
        color,
        size,
    );
}

pub fn header_height(ui: &Ui) -> u32 {
    ui.px(44.0)
}

/// The session's breadcrumb across the top of the terminal: project, session,
/// branch and changes, and the servers it started.
#[allow(clippy::too_many_arguments)]
pub fn draw_header(
    frame: &mut Frame<'_>,
    ui: &mut Ui,
    pal: &Palette,
    left: u32,
    project: &Project,
    title: &str,
    context: &Context,
) {
    let height = header_height(ui);
    let width = frame.width.saturating_sub(left);
    fill_rect(frame, left, 0, width, height, pal.canvas);
    fill_rect(frame, left, height.saturating_sub(1), width, 1, pal.border);
    let mid = height / 2;
    let body = ui.font(Face::Ui, 14.0, REGULAR);
    let strong = ui.font(Face::Ui, 14.0, SEMIBOLD);
    let small = ui.font(Face::Ui, 12.0, REGULAR);
    let gap = ui.px(8.0);
    let end = frame.width.saturating_sub(ui.px(16.0));
    let room = |x: u32| end.saturating_sub(x);

    let mut x = left + ui.px(16.0);
    monogram_tile(
        frame,
        ui,
        pal,
        x,
        mid,
        &project.name,
        project_color(project, pal),
    );
    x += ui.px(16.0) + gap;
    x += ui.text_mid(frame, x, mid, &project.name, body, pal.muted, room(x)) + gap;
    x += ui.text_mid(frame, x, mid, "/", body, pal.faint, room(x)) + gap;
    x += ui.text_mid(
        frame,
        x,
        mid,
        title,
        strong,
        pal.fg,
        room(x).min(ui.px(360.0)),
    );

    if let Some(git) = &context.git {
        x += ui.px(16.0);
        x += ui.text_mid(
            frame,
            x,
            mid,
            &git.head,
            small,
            pal.label,
            room(x).min(ui.px(240.0)),
        );
        for (n, prefix, color) in [
            (git.changed, "\u{b1}", pal.yellow),
            (git.ahead, "\u{2191}", pal.green),
            (git.behind, "\u{2193}", pal.red),
        ] {
            if n > 0 {
                x += ui.px(6.0);
                x += ui.text_mid(
                    frame,
                    x,
                    mid,
                    &format!("{prefix}{n}"),
                    small,
                    color,
                    room(x),
                );
            }
        }
    }
    for server in &context.servers {
        x += ui.px(16.0);
        ui::dot(frame, x + ui.px(3.0), mid, ui.px(6.0), pal.cyan);
        x += ui.px(12.0);
        x += ui.text_mid(
            frame,
            x,
            mid,
            &format!("{} :{}", server.name, server.port),
            small,
            pal.muted,
            room(x),
        );
    }
}

/// The find bar, floating at the top right of the terminal.
pub fn draw_find(
    frame: &mut Frame<'_>,
    ui: &mut Ui,
    pal: &Palette,
    top: u32,
    query: &str,
    count: &str,
) {
    let (w, h) = (ui.px(380.0), ui.px(36.0));
    let x = frame.width.saturating_sub(w + ui.px(16.0));
    let y = top + ui.px(8.0);
    ui::card(
        frame,
        x,
        y,
        w,
        h,
        ui.px(8.0),
        pal.raised,
        blend(pal.canvas, pal.fg, 40),
    );
    let mid = y + h / 2;
    ui.icon(frame, x + ui.px(16.0), mid, icon::SEARCH, 12.0, pal.label);
    let body = ui.font(Face::Ui, 14.0, REGULAR);
    let small = ui.font(Face::Ui, 12.0, REGULAR);
    let count_x = ui.text_right(frame, x + w - ui.px(12.0), mid, count, small, pal.label);
    let tx = x + ui.px(32.0);
    let room = count_x.saturating_sub(tx + ui.px(8.0));
    if query.is_empty() {
        let hint = "Find \u{b7} Enter previous, Shift+Enter next";
        ui.text_mid(frame, tx, mid, hint, body, pal.label, room);
    } else {
        let drawn = ui.text_mid(frame, tx, mid, query, body, pal.fg, room);
        let caret = ui.px(16.0);
        fill_rect(
            frame,
            tx + drawn + 1,
            mid - caret / 2,
            ui.px(1.0).max(1),
            caret,
            pal.accent,
        );
    }
}

/// Where the terminal's cells are, for drawing over them.
#[derive(Debug, Clone, Copy)]
pub struct TermArea {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub cell_height: u32,
}

/// How a command block ended, for the mark on its prompt row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BlockStatus {
    Running,
    Passed(Option<std::time::Duration>),
    Failed(i32, Option<std::time::Duration>),
    /// The shell did not say how it ended (cmd).
    Ended,
}

/// A command block's prompt row on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockMark {
    pub row: u32,
    /// The line of its prompt, which identifies it.
    pub prompt: u64,
    pub status: BlockStatus,
    pub hovered: bool,
}

/// What the buttons on a hovered block do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockAction {
    CopyOutput,
    Rerun,
}

/// `3.4 s`, `1m 05s`.
pub fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.1} s")
    } else {
        format!("{}m {:02}s", d.as_secs() / 60, d.as_secs() % 60)
    }
}

fn mark_label(status: BlockStatus) -> (String, Option<char>) {
    match status {
        BlockStatus::Running => ("running".to_owned(), None),
        BlockStatus::Passed(d) => (
            d.map(format_duration).unwrap_or_default(),
            Some(icon::CHECK),
        ),
        BlockStatus::Failed(code, d) => (
            match d {
                Some(d) => format!("exit {code} \u{b7} {}", format_duration(d)),
                None => format!("exit {code}"),
            },
            Some(icon::CROSS),
        ),
        BlockStatus::Ended => (String::new(), None),
    }
}

/// The chip at the right end of a prompt row: its rectangle, or `None` when
/// there is nothing to say.
fn mark_chip(ui: &mut Ui, area: &TermArea, mark: &BlockMark) -> Option<Rect> {
    let (label, glyph) = mark_label(mark.status);
    if label.is_empty() && glyph.is_none() {
        return None;
    }
    let small = ui.font(Face::Ui, 12.0, REGULAR);
    let lead = match (mark.status, glyph) {
        (BlockStatus::Running, _) => ui.px(14.0),
        (_, Some(_)) => ui.px(16.0),
        _ => 0,
    };
    let w = ui.measure(&label, small) + lead + ui.px(16.0);
    let h = ui.px(20.0).min(area.cell_height.max(ui.px(16.0)));
    let y = area.top + mark.row * area.cell_height + area.cell_height / 2 - h / 2;
    Some(Rect {
        x: area.right.saturating_sub(w),
        y,
        w,
        h,
    })
}

/// Where a block's buttons are: (Copy output, Rerun), when it can show them.
fn mark_buttons(
    ui: &mut Ui,
    area: &TermArea,
    mark: &BlockMark,
) -> Option<[(Rect, BlockAction); 2]> {
    if !mark.hovered || mark.status == BlockStatus::Running {
        return None;
    }
    let small = ui.font(Face::Ui, 12.0, REGULAR);
    let mut x = mark_chip(ui, area, mark).map_or(area.right, |c| c.x);
    let h = ui.px(20.0).min(area.cell_height.max(ui.px(16.0)));
    let y = area.top + mark.row * area.cell_height + area.cell_height / 2 - h / 2;
    let mut rects = Vec::new();
    for (text, action) in [
        ("Rerun", BlockAction::Rerun),
        ("Copy output", BlockAction::CopyOutput),
    ] {
        let w = ui.measure(text, small) + ui.px(16.0);
        x = x.saturating_sub(w + ui.px(6.0));
        rects.push((Rect { x, y, w, h }, action));
    }
    Some([rects[1], rects[0]])
}

/// The button of a hovered block under (`x`, `y`).
pub fn block_action_at(
    ui: &mut Ui,
    area: &TermArea,
    mark: &BlockMark,
    x: f64,
    y: f64,
) -> Option<BlockAction> {
    mark_buttons(ui, area, mark)?
        .into_iter()
        .find(|(r, _)| r.contains(x, y))
        .map(|(_, a)| a)
}

/// A hairline above each command, and its result as a chip at the right end
/// of its prompt row, drawn over any text that reaches that far.
pub fn draw_blocks(
    frame: &mut Frame<'_>,
    ui: &mut Ui,
    pal: &Palette,
    area: &TermArea,
    marks: &[BlockMark],
) {
    let small = ui.font(Face::Ui, 12.0, REGULAR);
    let edge = blend(pal.canvas, pal.fg, 34);
    for mark in marks {
        let y = area.top + mark.row * area.cell_height;
        if mark.row > 0 {
            fill_rect(
                frame,
                area.left,
                y,
                area.right.saturating_sub(area.left),
                1,
                pal.border,
            );
        }
        if let Some(chip) = mark_chip(ui, area, mark) {
            ui::card(
                frame,
                chip.x,
                chip.y,
                chip.w,
                chip.h,
                chip.h / 2,
                pal.raised,
                edge,
            );
            let (label, glyph) = mark_label(mark.status);
            let mid = chip.mid();
            let mut x = chip.x + ui.px(8.0);
            let failed = matches!(mark.status, BlockStatus::Failed(..));
            match (mark.status, glyph) {
                (BlockStatus::Running, _) => {
                    ui::dot(frame, x + ui.px(3.0), mid, ui.px(6.0), pal.blue);
                    x += ui.px(14.0);
                }
                (_, Some(glyph)) => {
                    let color = if failed { pal.red } else { pal.green };
                    ui.icon(frame, x + ui.px(5.0), mid, glyph, 9.0, color);
                    x += ui.px(16.0);
                }
                _ => {}
            }
            let color = if failed { pal.red } else { pal.label };
            ui.text_mid(frame, x, mid, &label, small, color, chip.w);
        }
        if let Some(buttons) = mark_buttons(ui, area, mark) {
            for (r, action) in buttons {
                ui::card(frame, r.x, r.y, r.w, r.h, r.h / 2, pal.raised, edge);
                let text = match action {
                    BlockAction::CopyOutput => "Copy output",
                    BlockAction::Rerun => "Rerun",
                };
                ui.text_mid(frame, r.x + ui.px(8.0), r.mid(), text, small, pal.fg, r.w);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(cwd: &str, root: Option<&str>) -> Context {
        Context {
            cwd: Some(PathBuf::from(cwd)),
            root: root.map(PathBuf::from),
            ..Context::default()
        }
    }

    #[test]
    fn sessions_group_by_repository_then_folder() {
        let home = Path::new(r"C:\Users\me");
        let a = ctx(r"C:\code\aeon", Some(r"C:\code\aeon"));
        let b = ctx(r"C:\Users\me", None);
        let c = ctx(r"C:\code\aeon\crates\aeon", Some(r"C:\code\aeon"));
        let d = ctx(r"C:\tmp", None);
        let e = Context::default();
        let projects = projects(&[&a, &b, &c, &d, &e], Some(home));
        let summary: Vec<(&str, &[usize])> = projects
            .iter()
            .map(|p| (p.name.as_str(), p.sessions.as_slice()))
            .collect();
        assert_eq!(
            summary,
            [
                ("aeon", &[0, 2][..]),
                ("Home", &[1, 4][..]),
                ("tmp", &[3][..])
            ]
        );
        assert_eq!(display_order(&projects), [0, 2, 1, 4, 3]);
    }

    #[test]
    fn monograms_take_word_initials() {
        assert_eq!(monogram("web-app"), "WA");
        assert_eq!(monogram("aeon"), "AE");
        assert_eq!(monogram("my_cool project"), "MP");
        assert_eq!(monogram("x"), "X");
        assert_eq!(monogram("--"), "?");
    }

    #[test]
    fn titles_say_where_the_session_is_in_its_project() {
        let home = Path::new(r"C:\Users\me");
        let project = Project {
            name: "aeon".into(),
            path: Some(PathBuf::from(r"C:\code\aeon")),
            sessions: vec![0],
        };
        let top = ctx(r"C:\code\aeon", Some(r"C:\code\aeon"));
        let deep = ctx(r"C:\code\aeon\crates\aeon", Some(r"C:\code\aeon"));
        assert_eq!(session_title(&top, "pwsh", &project, Some(home)), "pwsh");
        assert_eq!(
            session_title(&deep, "pwsh", &project, Some(home)),
            r"crates\aeon"
        );
        let homeless = Project {
            name: "Home".into(),
            path: None,
            sessions: vec![0],
        };
        assert_eq!(
            session_title(&ctx(r"C:\Users\me", None), "cmd", &homeless, Some(home)),
            "cmd"
        );
    }

    #[test]
    fn the_sidebar_hit_tests_what_it_draws() {
        let mut ui = Ui::new(1.0);
        let projects = vec![Project {
            name: "aeon".into(),
            path: Some(PathBuf::from(r"C:\code\aeon")),
            sessions: vec![0, 1],
        }];
        let sb = Sidebar::new(&mut ui, 256, 800, &projects, Some(&Pill::Available));
        assert_eq!(sb.hit(100.0, 50.0), Some(Hit::NewSession));
        // New session 40..72, gap to 78, project 78..110, sessions 110..146..182.
        assert_eq!(sb.hit(100.0, 90.0), None);
        assert_eq!(sb.hit(100.0, 120.0), Some(Hit::Session(0)));
        assert_eq!(sb.hit(100.0, 160.0), Some(Hit::Session(1)));
        assert_eq!(sb.hit(20.0, 770.0), Some(Hit::Settings));
        assert_eq!(sb.hit(240.0, 770.0), Some(Hit::Update));
    }
}
