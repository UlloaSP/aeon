//! The sidebar and the session header, laid out like T3 Code: a list of
//! sessions, each with its state as a small dot beside its name.

use std::path::{Path, PathBuf};

use aeon_context::{Context, Git, Task};

use crate::render::{Frame, blend, fill_rect};
use crate::sys::Face;
use crate::ui::{self, Palette, REGULAR, SEMIBOLD, Ui, icon};

/// Sessions that run in the same repository, or the same folder outside one:
/// what the project panel and notifications speak of.
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

/// What a session is called: its repository when it is in one, otherwise
/// its shell, such as `pwsh` or the WSL distro.
pub fn session_title(context: &Context, shell: &str) -> String {
    context
        .root
        .as_deref()
        .and_then(Path::file_name)
        .map_or_else(|| shell.to_owned(), |n| n.to_string_lossy().into_owned())
}

/// Where a session is, for the header: its folder, with the home folder as
/// `~`.
pub fn place(context: &Context, home: Option<&Path>) -> String {
    let Some(cwd) = context.cwd.as_deref() else {
        return String::new();
    };
    match home.and_then(|h| cwd.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~\\{}", rest.display()),
        None => cwd.display().to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
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
    /// New session: the menu of shells, distros, and hosts.
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
    pub fn new(ui: &mut Ui, width: u32, height: u32, sessions: usize, pill: Option<&Pill>) -> Self {
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
        for session in 0..sessions {
            push(&mut y, ui.px(50.0), Row::Session(session));
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
            .map(|(rect, row)| {
                let end = x >= f64::from(rect.x + rect.w - self.mute);
                match row {
                    Row::NewSession => Hit::NewSession,
                    Row::Session(i) if end => Hit::Mute(*i),
                    Row::Session(i) => Hit::Session(*i),
                }
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
    /// Its shell ("PowerShell", "WSL · kali-linux"), or its folder when the
    /// title is the shell.
    pub detail: String,
    pub state: RowState,
    /// It finished, failed, or rang while you were elsewhere: drawn bold
    /// until you look at it.
    pub unseen: bool,
    pub muted: bool,
    /// How many panes the session is split into.
    pub panes: usize,
    /// The title is being renamed: drawn as a field with a caret.
    pub editing: bool,
    /// The whole title is selected while renaming: typing replaces it.
    pub selected: bool,
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
                let right = rect.x + rect.w - ui.px(10.0);
                ui.text_right(frame, right, mid, state.new_keys, small, pal.label);
            }
            Row::Session(i) => {
                let Some(session) = state.sessions.get(i) else {
                    continue;
                };
                let selected = i == state.active && !state.settings_open;
                let hovered = state.hover == Some(Hit::Session(i)) && !session.editing;
                if session.editing {
                    ui::card(
                        frame, rect.x, rect.y, rect.w, rect.h, radius, pal.raised, pal.accent,
                    );
                } else if selected {
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
                // Two lines: the title, and under it the shell or folder.
                let line = mid.saturating_sub(ui.px(8.0));
                let below = mid + ui.px(10.0);
                let slot = rect.x + ui.px(18.0);
                let (dot, meta, meta_color) = match session.state {
                    RowState::Idle if session.panes > 1 => {
                        (None, format!("{} panes", session.panes), pal.label)
                    }
                    RowState::Idle => (None, String::new(), pal.label),
                    RowState::Running => (Some(pal.blue), "running".to_owned(), pal.label),
                    RowState::Server(port) => (Some(pal.cyan), format!(":{port}"), pal.cyan),
                    RowState::Progress(p) => (Some(pal.blue), format!("{p}%"), pal.label),
                    RowState::Failed(code) => (Some(pal.red), format!("exit {code}"), pal.red),
                    RowState::Done => (Some(pal.green), "done".to_owned(), pal.green),
                    RowState::Bell => (Some(pal.yellow), "bell".to_owned(), pal.yellow),
                };
                match dot {
                    Some(color) => ui::dot(frame, slot, line, ui.px(9.0), color),
                    None => {
                        let strength = if selected || hovered { 215 } else { 102 };
                        ui.icon(
                            frame,
                            slot,
                            line,
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
                    ui.icon(frame, cx, line, glyph, 12.0, color);
                    cx.saturating_sub(ui.px(10.0))
                } else {
                    let mut end = right;
                    if !meta.is_empty() {
                        end = ui.text_right(frame, end, line, &meta, small, meta_color);
                    }
                    if session.muted {
                        let cx = end.saturating_sub(ui.px(10.0));
                        ui.icon(frame, cx, line, icon::MUTED, 10.0, pal.label);
                        end = cx.saturating_sub(ui.px(8.0));
                    }
                    end
                };
                let x = rect.x + ui.px(36.0);
                let font = if selected || session.unseen {
                    strong
                } else {
                    body
                };
                let room = meta_x.saturating_sub(x + ui.px(8.0));
                if session.editing && session.selected && !session.title.is_empty() {
                    let w = ui.measure(&session.title, font).min(room);
                    let h = ui.px(20.0);
                    fill_rect(
                        frame,
                        x.saturating_sub(ui.px(2.0)),
                        line.saturating_sub(h / 2),
                        w + ui.px(4.0),
                        h,
                        blend(pal.raised, pal.accent, 90),
                    );
                }
                let width = ui.text_mid(frame, x, line, &session.title, font, pal.fg, room);
                let detail_room = (rect.x + rect.w).saturating_sub(x + ui.px(10.0));
                ui.text_mid(
                    frame,
                    x,
                    below,
                    &session.detail,
                    small,
                    pal.label,
                    detail_room,
                );
                if session.editing {
                    let caret = ui.px(16.0);
                    fill_rect(
                        frame,
                        x + width.min(room) + 1,
                        line.saturating_sub(caret / 2),
                        ui.px(1.0).max(1),
                        caret,
                        pal.accent,
                    );
                }
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

pub fn header_height(ui: &Ui) -> u32 {
    ui.px(44.0)
}

/// What a button in the session header does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderHit {
    OpenEditor,
    TogglePanel,
}

/// The header's buttons, from the right: the project panel toggle, then
/// "Open in VS Code" when VS Code is installed; just its icon when the header
/// starting at `left` is narrow, so the session's title keeps its room.
pub fn header_buttons(
    ui: &mut Ui,
    left: u32,
    frame_width: u32,
    editor: bool,
) -> Vec<(Rect, HeaderHit)> {
    let height = header_height(ui);
    let h = ui.px(28.0);
    let y = (height - h) / 2;
    let mut x = frame_width.saturating_sub(ui.px(12.0));
    let mut out = Vec::new();
    let w = ui.px(32.0);
    x = x.saturating_sub(w);
    out.push((Rect { x, y, w, h }, HeaderHit::TogglePanel));
    if editor {
        let font = ui.font(Face::Ui, 13.0, REGULAR);
        let w = if frame_width.saturating_sub(left) < ui.px(720.0) {
            ui.px(32.0)
        } else {
            ui.measure("Open in VS Code", font) + ui.px(36.0)
        };
        x = x.saturating_sub(w + ui.px(4.0));
        out.push((Rect { x, y, w, h }, HeaderHit::OpenEditor));
    }
    out
}

/// The session across the top of the terminal: its title, where it is, its
/// branch and changes, and the servers it started; its buttons at the right.
#[allow(clippy::too_many_arguments)]
pub fn draw_header(
    frame: &mut Frame<'_>,
    ui: &mut Ui,
    pal: &Palette,
    left: u32,
    title: &str,
    shell: Option<&str>,
    place: &str,
    context: &Context,
    buttons: &[(Rect, HeaderHit)],
    hover: Option<HeaderHit>,
    panel_open: bool,
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
    let end = buttons
        .iter()
        .map(|(r, _)| r.x)
        .min()
        .unwrap_or(frame.width)
        .saturating_sub(ui.px(16.0));
    let room = |x: u32| end.saturating_sub(x);

    for (r, hit) in buttons {
        let hovered = hover == Some(*hit);
        let on = *hit == HeaderHit::TogglePanel && panel_open;
        if on {
            ui::round_rect(frame, r.x, r.y, r.w, r.h, ui.px(6.0), pal.highlight);
        } else if hovered {
            ui::round_rect(frame, r.x, r.y, r.w, r.h, ui.px(6.0), pal.hover);
        }
        let color = if hovered || on { pal.fg } else { pal.muted };
        match hit {
            HeaderHit::TogglePanel => {
                ui.icon(frame, r.x + r.w / 2, r.mid(), icon::PANEL, 14.0, color)
            }
            HeaderHit::OpenEditor if r.w <= ui.px(32.0) => {
                ui.icon(frame, r.x + r.w / 2, r.mid(), icon::CODE, 13.0, color)
            }
            HeaderHit::OpenEditor => {
                ui.icon(frame, r.x + ui.px(14.0), r.mid(), icon::CODE, 13.0, color);
                let font = ui.font(Face::Ui, 13.0, REGULAR);
                ui.text_mid(
                    frame,
                    r.x + ui.px(28.0),
                    r.mid(),
                    "Open in VS Code",
                    font,
                    color,
                    r.w,
                );
            }
        }
    }

    let mut x = left + ui.px(16.0);
    x += ui.text_mid(
        frame,
        x,
        mid,
        title,
        strong,
        pal.fg,
        room(x).min(ui.px(360.0)),
    );
    if let Some(shell) = shell
        && room(x) > ui.px(80.0)
    {
        x += gap + ui.px(4.0);
        ui.icon(frame, x + ui.px(6.0), mid, icon::TERMINAL, 12.0, pal.label);
        x += ui.px(18.0);
        x += ui.text_mid(
            frame,
            x,
            mid,
            shell,
            body,
            pal.muted,
            room(x).min(ui.px(240.0)),
        );
    }
    if !place.is_empty() {
        x += gap + ui.px(4.0);
        x += ui.text_mid(
            frame,
            x,
            mid,
            place,
            body,
            pal.muted,
            room(x).min(ui.px(420.0)),
        );
    }

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
            if n > 0 && room(x) > ui.px(24.0) {
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
        if room(x) < ui.px(60.0) {
            break;
        }
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
    right: u32,
    query: &str,
    count: &str,
) {
    let (w, h) = (ui.px(380.0), ui.px(36.0));
    let x = right.saturating_sub(w + ui.px(8.0));
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
}

/// A command block's prompt row on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockMark {
    pub row: u32,
    /// Rows the block covers on screen, from its prompt row; at least 1.
    pub span: u32,
    /// An empty row above the prompt and below the output: the block's edges
    /// run through their middles, for room inside it.
    pub gap_above: bool,
    pub gap_below: bool,
    /// The line of its prompt, which identifies it.
    pub prompt: u64,
    pub status: BlockStatus,
    pub hovered: bool,
}

/// The margin left of the terminal's text, for a block's bar and its copy
/// button, in logical pixels.
pub const GUTTER: f64 = 22.0;

/// The command blocks on screen: a mark for each prompt in view, and the
/// failed ones' tinted spans in half lines (`2 * line`), so a tint can stop
/// in the middle of an empty row as the block's edge does.
pub fn block_marks(
    term: &aeon_term::Terminal,
    scroll: usize,
    took: &dyn Fn(u64) -> Option<std::time::Duration>,
    hovered: Option<u64>,
) -> (Vec<BlockMark>, Vec<(u64, u64)>) {
    if term.modes().alt_screen {
        return (Vec::new(), Vec::new());
    }
    let first = term.screen_line().saturating_sub(scroll as u64);
    let rows = term.grid().rows() as u64;
    let blank = |line: u64| {
        term.line(line)
            .is_some_and(|cells| cells.iter().all(|c| c.ch == ' '))
    };
    let mut marks = Vec::new();
    let mut tints = Vec::new();
    for block in term.blocks() {
        if block.command.is_empty() || block.output.is_none() {
            continue;
        }
        let gap_above = block.prompt > 0 && blank(block.prompt - 1);
        let gap_below = block.end.is_some_and(blank);
        let status = match (block.end, block.exit) {
            (None, _) => BlockStatus::Running,
            // cmd cannot say how a command ended: it shows as passed.
            (Some(_), Some(0) | None) => BlockStatus::Passed(took(block.id)),
            (Some(end), Some(code)) => {
                tints.push((
                    2 * block.prompt - u64::from(gap_above),
                    2 * end + u64::from(gap_below),
                ));
                BlockStatus::Failed(code, took(block.id))
            }
        };
        if (first..first + rows).contains(&block.prompt) {
            let end = block.end.unwrap_or(first + rows).min(first + rows);
            marks.push(BlockMark {
                row: (block.prompt - first) as u32,
                span: end.saturating_sub(block.prompt).max(1) as u32,
                gap_above,
                gap_below: gap_below && end < first + rows,
                prompt: block.prompt,
                status,
                hovered: hovered == Some(block.prompt),
            });
        }
    }
    (marks, tints)
}

/// Where a block starts and ends on screen: the middles of the empty rows
/// around it, or its first and last rows' edges.
fn block_edges(area: &TermArea, mark: &BlockMark) -> (u32, u32) {
    let half = area.cell_height / 2;
    let top = area.top + mark.row * area.cell_height;
    let bottom = top + mark.span * area.cell_height;
    (
        top.saturating_sub(if mark.gap_above { half } else { 0 })
            .max(area.top),
        bottom + if mark.gap_below { half } else { 0 },
    )
}

/// The copy-command button in the margin left of a hovered block's prompt.
fn copy_button(ui: &mut Ui, area: &TermArea, mark: &BlockMark) -> Option<Rect> {
    if !mark.hovered {
        return None;
    }
    let size = ui.px(20.0).min(area.cell_height.max(ui.px(16.0)));
    let mid = area.top + mark.row * area.cell_height + area.cell_height / 2;
    Some(Rect {
        x: area.left + ui.px(GUTTER) / 2 + ui.px(2.0) - size / 2,
        y: mid - size / 2,
        w: size,
        h: size,
    })
}

/// What the buttons on a hovered block do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockAction {
    CopyCommand,
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

fn mark_label(status: BlockStatus) -> String {
    match status {
        BlockStatus::Running => "running".to_owned(),
        BlockStatus::Passed(d) => d.map(format_duration).unwrap_or_default(),
        BlockStatus::Failed(code, Some(d)) => format!("exit {code} \u{b7} {}", format_duration(d)),
        BlockStatus::Failed(code, None) => format!("exit {code}"),
    }
}

/// Where a block's result is written at the right end of its prompt row,
/// or `None` when there is nothing to say.
fn mark_text(ui: &mut Ui, area: &TermArea, mark: &BlockMark) -> Option<Rect> {
    let label = mark_label(mark.status);
    if label.is_empty() {
        return None;
    }
    let small = ui.font(Face::Ui, 12.0, REGULAR);
    let lead = if mark.status == BlockStatus::Running {
        ui.px(12.0)
    } else {
        0
    };
    let w = ui.measure(&label, small) + lead;
    let h = area.cell_height;
    Some(Rect {
        x: area.right.saturating_sub(w),
        y: area.top + mark.row * area.cell_height,
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
    let mut x = mark_text(ui, area, mark).map_or(area.right, |t| t.x.saturating_sub(ui.px(6.0)));
    let h = ui.px(20.0).min(area.cell_height.max(ui.px(16.0)));
    let y = area.top + mark.row * area.cell_height + area.cell_height / 2 - h / 2;
    let mut rects = Vec::new();
    for (text, action) in [
        ("Rerun", BlockAction::Rerun),
        ("Copy output", BlockAction::CopyOutput),
    ] {
        let w = ui.measure(text, small) + ui.px(14.0);
        x = x.saturating_sub(w + ui.px(4.0));
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
    if copy_button(ui, area, mark).is_some_and(|r| r.contains(x, y)) {
        return Some(BlockAction::CopyCommand);
    }
    mark_buttons(ui, area, mark)?
        .into_iter()
        .find(|(r, _)| r.contains(x, y))
        .map(|(_, a)| a)
}

/// A hairline above each command, through the empty row before its prompt;
/// its result as quiet text at the right end of its prompt row; down the
/// left of a failed one, a red bar; and on the one under the mouse, buttons to
/// copy its command (left) and its output or run it again (right).
pub fn draw_blocks(
    frame: &mut Frame<'_>,
    ui: &mut Ui,
    pal: &Palette,
    area: &TermArea,
    marks: &[BlockMark],
) {
    let small = ui.font(Face::Ui, 12.0, REGULAR);
    for mark in marks {
        let (top, bottom) = block_edges(area, mark);
        if top > area.top {
            fill_rect(
                frame,
                area.left,
                top,
                area.right.saturating_sub(area.left),
                1,
                pal.border,
            );
        }
        let failed = matches!(mark.status, BlockStatus::Failed(..));
        if failed {
            fill_rect(
                frame,
                area.left + ui.px(4.0),
                top + 1,
                ui.px(3.0).max(2),
                bottom.saturating_sub(top + 1),
                pal.red,
            );
        }
        if let Some(r) = copy_button(ui, area, mark) {
            ui.icon(
                frame,
                r.x + r.w / 2,
                r.y + r.h / 2,
                icon::COPY,
                12.0,
                pal.muted,
            );
        }
        if let Some(t) = mark_text(ui, area, mark) {
            let mid = t.y + t.h / 2;
            let mut x = t.x;
            if mark.status == BlockStatus::Running {
                ui::dot(frame, x + ui.px(3.0), mid, ui.px(6.0), pal.blue);
                x += ui.px(12.0);
            }
            let color = match mark.status {
                BlockStatus::Failed(..) => pal.red,
                BlockStatus::Running => pal.blue,
                _ => pal.label,
            };
            ui.text_mid(frame, x, mid, &mark_label(mark.status), small, color, t.w);
        }
        if let Some(buttons) = mark_buttons(ui, area, mark) {
            for (r, action) in buttons {
                ui::round_rect(frame, r.x, r.y, r.w, r.h, ui.px(5.0), pal.highlight);
                let text = match action {
                    BlockAction::CopyOutput => "Copy output",
                    BlockAction::Rerun | BlockAction::CopyCommand => "Rerun",
                };
                ui.text_mid(frame, r.x + ui.px(7.0), r.mid(), text, small, pal.fg, r.w);
            }
        }
    }
}

/// A server in the project panel: which session started it.
pub struct PanelServer {
    pub name: String,
    pub port: u16,
    pub session: String,
}

/// What the project panel shows.
pub struct PanelData<'a> {
    pub tasks: &'a [Task],
    /// For each task, whether a session is running it.
    pub running: &'a [bool],
    pub servers: &'a [PanelServer],
    pub git: Option<&'a Git>,
}

/// Something in the project panel a click acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelHit {
    Run(usize),
    Stop(usize),
    OpenServer(usize),
    StopServer(usize),
}

#[derive(Debug, Clone, PartialEq)]
enum PanelItem {
    Section(&'static str, String),
    Empty(&'static str),
    Task(usize),
    Server(usize),
    File(usize),
}

/// The project panel at the right of the terminal: tasks, servers, git.
/// Its layout is shared by drawing and hit-testing.
pub struct Panel {
    pub x: u32,
    pub width: u32,
    top: u32,
    items: Vec<(Rect, PanelItem)>,
    buttons: Vec<(Rect, PanelHit)>,
}

pub const PANEL_WIDTH: f64 = 320.0;

impl Panel {
    pub fn new(
        ui: &mut Ui,
        x: u32,
        width: u32,
        top: u32,
        height: u32,
        data: &PanelData<'_>,
    ) -> Self {
        let inset = ui.px(8.0);
        let w = width.saturating_sub(2 * inset);
        let left = x + inset;
        let mut items = Vec::new();
        let mut buttons = Vec::new();
        let mut y = top + ui.px(4.0);
        let small = ui.font(Face::Ui, 12.0, REGULAR);
        let mut push = |y: &mut u32, h: u32, item: PanelItem| {
            if *y + h <= height {
                items.push((
                    Rect {
                        x: left,
                        y: *y,
                        w,
                        h,
                    },
                    item,
                ));
            }
            *y += h;
        };
        let source = data
            .tasks
            .first()
            .map_or(String::new(), |t| t.source.to_owned());
        push(&mut y, ui.px(32.0), PanelItem::Section("Tasks", source));
        if data.tasks.is_empty() {
            push(
                &mut y,
                ui.px(32.0),
                PanelItem::Empty("No package.json, justfile, Makefile, or Cargo.toml here."),
            );
        }
        for i in 0..data.tasks.len() {
            push(&mut y, ui.px(32.0), PanelItem::Task(i));
        }
        y += ui.px(8.0);
        push(
            &mut y,
            ui.px(32.0),
            PanelItem::Section("Servers", String::new()),
        );
        if data.servers.is_empty() {
            push(
                &mut y,
                ui.px(32.0),
                PanelItem::Empty("None. Ports this project's sessions open show up here."),
            );
        }
        for i in 0..data.servers.len() {
            push(&mut y, ui.px(40.0), PanelItem::Server(i));
        }
        if let Some(git) = data.git {
            y += ui.px(8.0);
            let mut aside = git.head.clone();
            if git.ahead > 0 {
                aside.push_str(&format!(" \u{2191}{}", git.ahead));
            }
            if git.behind > 0 {
                aside.push_str(&format!(" \u{2193}{}", git.behind));
            }
            push(&mut y, ui.px(32.0), PanelItem::Section("Git", aside));
            if git.files.is_empty() {
                push(&mut y, ui.px(28.0), PanelItem::Empty("No changes."));
            }
            for i in 0..git.files.len() {
                push(&mut y, ui.px(26.0), PanelItem::File(i));
            }
        }
        for (rect, item) in &items {
            let mut right = rect.x + rect.w - ui.px(8.0);
            let mut button = |label: &str, hit: PanelHit, right: &mut u32| {
                let bw = ui.measure(label, small) + ui.px(16.0);
                let bh = ui.px(24.0);
                *right = right.saturating_sub(bw);
                buttons.push((
                    Rect {
                        x: *right,
                        y: rect.mid() - bh / 2,
                        w: bw,
                        h: bh,
                    },
                    hit,
                ));
                *right = right.saturating_sub(ui.px(6.0));
            };
            match item {
                PanelItem::Task(i) => {
                    if data.running.get(*i).copied().unwrap_or(false) {
                        button("Stop", PanelHit::Stop(*i), &mut right);
                    } else {
                        button("Run", PanelHit::Run(*i), &mut right);
                    }
                }
                PanelItem::Server(i) => {
                    button("Stop", PanelHit::StopServer(*i), &mut right);
                    button("Open", PanelHit::OpenServer(*i), &mut right);
                }
                _ => {}
            }
        }
        Self {
            x,
            width,
            top,
            items,
            buttons,
        }
    }

    pub fn hit(&self, x: f64, y: f64) -> Option<PanelHit> {
        self.buttons
            .iter()
            .find(|(r, _)| r.contains(x, y))
            .map(|(_, h)| *h)
    }
}

pub fn draw_panel(
    frame: &mut Frame<'_>,
    ui: &mut Ui,
    pal: &Palette,
    panel: &Panel,
    data: &PanelData<'_>,
    hover: Option<PanelHit>,
) {
    fill_rect(
        frame,
        panel.x,
        panel.top,
        panel.width,
        frame.height.saturating_sub(panel.top),
        pal.canvas,
    );
    fill_rect(
        frame,
        panel.x,
        panel.top,
        1,
        frame.height.saturating_sub(panel.top),
        pal.border,
    );
    let label = ui.font(Face::Ui, 12.0, SEMIBOLD);
    let small = ui.font(Face::Ui, 12.0, REGULAR);
    let code = ui.font(Face::Code, 12.0, REGULAR);
    let code_strong = ui.font(Face::Code, 12.0, SEMIBOLD);
    for (rect, item) in &panel.items {
        let mid = rect.mid();
        let x = rect.x + ui.px(8.0);
        let right = panel
            .buttons
            .iter()
            .filter(|(r, _)| r.y + r.h > rect.y && r.y < rect.y + rect.h)
            .map(|(r, _)| r.x)
            .min()
            .unwrap_or(rect.x + rect.w)
            .saturating_sub(ui.px(8.0));
        match item {
            PanelItem::Section(title, aside) => {
                let tw = ui.text_mid(frame, x, mid, title, label, pal.label, rect.w);
                let ax = if aside.is_empty() {
                    rect.x + rect.w
                } else {
                    ui.text_right(
                        frame,
                        rect.x + rect.w - ui.px(8.0),
                        mid,
                        aside,
                        small,
                        pal.label,
                    )
                };
                let line_x = x + tw + ui.px(8.0);
                if ax > line_x + ui.px(16.0) {
                    fill_rect(frame, line_x, mid, ax - line_x - ui.px(8.0), 1, pal.border);
                }
            }
            PanelItem::Empty(text) => {
                ui.text_mid(
                    frame,
                    x,
                    mid,
                    text,
                    small,
                    pal.label,
                    rect.w.saturating_sub(ui.px(16.0)),
                );
            }
            PanelItem::Task(i) => {
                let task = &data.tasks[*i];
                let running = data.running.get(*i).copied().unwrap_or(false);
                if running {
                    ui::dot(frame, x + ui.px(3.0), mid, ui.px(6.0), pal.blue);
                }
                let nx = x + ui.px(14.0);
                let nw = ui.text_mid(
                    frame,
                    nx,
                    mid,
                    &task.name,
                    code_strong,
                    pal.fg,
                    ui.px(110.0),
                );
                let dx = nx + nw.max(ui.px(48.0)) + ui.px(10.0);
                let detail = if task.detail.is_empty() {
                    &task.command
                } else {
                    &task.detail
                };
                ui.text_mid(
                    frame,
                    dx,
                    mid,
                    detail,
                    code,
                    pal.label,
                    right.saturating_sub(dx),
                );
            }
            PanelItem::Server(i) => {
                let server = &data.servers[*i];
                ui::dot(
                    frame,
                    x + ui.px(3.0),
                    mid - ui.px(7.0),
                    ui.px(6.0),
                    pal.cyan,
                );
                let nx = x + ui.px(14.0);
                let nw = ui.text_mid(
                    frame,
                    nx,
                    mid - ui.px(7.0),
                    &server.name,
                    code_strong,
                    pal.fg,
                    ui.px(100.0),
                );
                ui.text_mid(
                    frame,
                    nx + nw + ui.px(6.0),
                    mid - ui.px(7.0),
                    &format!(":{}", server.port),
                    code,
                    pal.cyan,
                    ui.px(60.0),
                );
                ui.text_mid(
                    frame,
                    nx,
                    mid + ui.px(9.0),
                    &format!("from {}", server.session),
                    small,
                    pal.label,
                    right.saturating_sub(nx),
                );
            }
            PanelItem::File(i) => {
                let Some(file) = data.git.and_then(|g| g.files.get(*i)) else {
                    continue;
                };
                let color = match file.status {
                    'A' | '?' => pal.green,
                    'D' => pal.red,
                    'U' => pal.magenta,
                    _ => pal.yellow,
                };
                let status = if file.status == '?' { 'U' } else { file.status };
                ui.text_mid(
                    frame,
                    x,
                    mid,
                    &status.to_string(),
                    code_strong,
                    color,
                    ui.px(16.0),
                );
                ui.text_mid(
                    frame,
                    x + ui.px(18.0),
                    mid,
                    &file.path,
                    code,
                    pal.fg,
                    rect.w.saturating_sub(ui.px(34.0)),
                );
            }
        }
    }
    let edge = blend(pal.canvas, pal.fg, 34);
    for (r, hit) in &panel.buttons {
        let hovered = hover == Some(*hit);
        let (text, danger) = match hit {
            PanelHit::Run(_) => ("Run", false),
            PanelHit::Stop(_) | PanelHit::StopServer(_) => ("Stop", true),
            PanelHit::OpenServer(_) => ("Open", false),
        };
        let fill = if hovered { pal.highlight } else { pal.raised };
        ui::card(
            frame,
            r.x,
            r.y,
            r.w,
            r.h,
            ui.px(6.0),
            fill,
            if danger {
                blend(pal.canvas, pal.red, 90)
            } else {
                edge
            },
        );
        ui.text_mid(
            frame,
            r.x + ui.px(8.0),
            r.mid(),
            text,
            small,
            if danger { pal.red } else { pal.fg },
            r.w,
        );
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
        let summary: Vec<(String, Vec<usize>)> = projects(&[&a, &b, &c, &d, &e], Some(home))
            .into_iter()
            .map(|p| (p.name, p.sessions))
            .collect();
        assert_eq!(
            summary,
            [
                ("aeon".to_owned(), vec![0, 2]),
                ("Home".to_owned(), vec![1, 4]),
                ("tmp".to_owned(), vec![3]),
            ]
        );
    }

    #[test]
    fn a_session_is_named_after_its_repository_or_its_shell() {
        let deep = ctx(r"C:\code\aeon\crates\aeon", Some(r"C:\code\aeon"));
        assert_eq!(session_title(&deep, "pwsh"), "aeon");
        assert_eq!(
            session_title(&ctx(r"C:\tmp", None), "kali-linux"),
            "kali-linux"
        );
        assert_eq!(session_title(&Context::default(), "cmd"), "cmd");
    }

    #[test]
    fn the_header_shows_home_as_a_tilde() {
        let home = Some(Path::new(r"C:\Users\me"));
        assert_eq!(place(&ctx(r"C:\Users\me", None), home), "~");
        assert_eq!(place(&ctx(r"C:\Users\me\code", None), home), r"~\code");
        assert_eq!(place(&ctx(r"D:\work", None), home), r"D:\work");
        assert_eq!(place(&Context::default(), home), "");
    }

    #[test]
    fn the_sidebar_hit_tests_what_it_draws() {
        let mut ui = Ui::new(1.0);
        let sb = Sidebar::new(&mut ui, 256, 800, 2, Some(&Pill::Available));
        assert_eq!(sb.hit(100.0, 50.0), Some(Hit::NewSession));
        assert_eq!(sb.hit(240.0, 50.0), Some(Hit::NewSession));
        // New session 40..72, a gap to 78, then sessions 78..128..178.
        assert_eq!(sb.hit(100.0, 90.0), Some(Hit::Session(0)));
        assert_eq!(sb.hit(240.0, 90.0), Some(Hit::Mute(0)));
        assert_eq!(sb.hit(100.0, 135.0), Some(Hit::Session(1)));
        assert_eq!(sb.hit(100.0, 185.0), None);
        assert_eq!(sb.hit(20.0, 770.0), Some(Hit::Settings));
        assert_eq!(sb.hit(240.0, 770.0), Some(Hit::Update));
    }
}
