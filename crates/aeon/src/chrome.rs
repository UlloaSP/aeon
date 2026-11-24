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
            .and_then(|(_, row)| match row {
                Row::NewSession => Some(Hit::NewSession),
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
pub struct SessionRow<'a> {
    pub context: &'a Context,
    pub title: String,
}

pub struct SidebarState<'a> {
    pub projects: &'a [Project],
    pub sessions: &'a [SessionRow<'a>],
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
                let server = session.context.servers.first();
                if server.is_some() {
                    ui::dot(frame, slot, mid, ui.px(9.0), pal.cyan);
                } else {
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
                let right = rect.x + rect.w - ui.px(10.0);
                let meta_x = match server {
                    Some(server) => ui.text_right(
                        frame,
                        right,
                        mid,
                        &format!(":{}", server.port),
                        small,
                        pal.cyan,
                    ),
                    None => right,
                };
                let x = rect.x + ui.px(60.0);
                let font = if selected { strong } else { body };
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
