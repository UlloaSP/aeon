use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Write};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use aeon_config::Config;
use aeon_context::{Context, Watcher};
use aeon_launcher::profiles::Kind;
use aeon_launcher::update::Status;
use aeon_launcher::{
    Action, Entry, Launcher, Profile, ResidentCommand, Sources, apps, history, sys, updater,
};
use aeon_pty::{Pty, PtySize};
use aeon_term::{Event as TermEvent, Match, MouseMode, Point, Terminal};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::{
    ElementState, KeyEvent, MouseButton, MouseScrollDelta, StartCause, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::platform::windows::IconExtWindows;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{CursorIcon, Icon, Window, WindowId};

use crate::chrome::{
    self, BlockAction, BlockMark, BlockStatus, HeaderHit, Hit, Panel, PanelData, PanelHit,
    PanelServer, Pill, Project, Rect, RowState, SessionRow, Sidebar, SidebarState, TermArea,
};
use crate::font::{self, Atlas};
use crate::input::{self, Button, MouseAction};
use crate::keys::Combo;
use crate::links::{self, Target};
use crate::palette::{self as finder, Content};
use crate::panes::{self, Direction, Split, Tree};
use crate::render::{self, Frame, Overlay, Theme};
use crate::settings::{self, Outcome, Settings};
use crate::ui::{Palette, Ui};

/// Identifies a session across threads; never reused within a process.
type SessionId = u64;

/// Sent from background threads to the UI thread.
pub enum Event {
    /// Where VS Code's `code` command is, looked up once off the UI thread.
    Editor(Option<PathBuf>),
    /// A chunk of shell output, from a session's PTY reader thread.
    Output(SessionId, Vec<u8>),
    /// A session's shell exited.
    Exited(SessionId),
    /// New context bar contents, from a session's watcher thread.
    Context(SessionId, Context),
    /// What the palette searches beyond the window, loaded the first time it
    /// opens.
    Catalog(Catalog),
}

/// Installed apps, programs on PATH, and new-session profiles.
pub struct Catalog {
    apps: Vec<aeon_launcher::App>,
    commands: HashSet<String>,
    profiles: Vec<Profile>,
}

/// How this window was started.
#[derive(Debug, Default)]
pub struct Launch {
    /// `--command`: a first command for the first shell (the launcher's
    /// Ctrl+Enter).
    pub run: Option<String>,
    /// `--shell` and `--arg`: the first session's shell, from a profile.
    pub program: Option<Program>,
    /// `--cwd`: where the first session starts.
    pub cwd: Option<PathBuf>,
}

/// A shell other than the configured one, chosen from a profile.
#[derive(Debug, Clone)]
pub struct Program {
    pub program: String,
    pub args: Vec<String>,
}

/// The apps list is reloaded when the palette opens and it is older than this.
const CATALOG_MAX_AGE: Duration = Duration::from_secs(10 * 60);
/// The palette's width over a window, in logical pixels.
const FINDER_WIDTH: f64 = 576.0;

pub struct App {
    proxy: EventLoopProxy<Event>,
    config: Config,
    config_error: Option<String>,
    theme: Theme,
    palette: Palette,
    /// The user's home folder: sessions there belong to no project.
    home: Option<PathBuf>,
    /// What the mouse is over in the sidebar, drawn highlighted.
    hover: Option<Hit>,
    /// How the window was started, until its first session opens.
    launch: Option<Launch>,
    /// The palette over the window, while open.
    finder: Option<Launcher>,
    /// Why the palette's last choice failed.
    finder_error: Option<String>,
    catalog: Option<Catalog>,
    catalog_requested: Option<Instant>,
    view: Option<View>,
    /// Every shell, in every pane of every session.
    sessions: Vec<Session>,
    /// Sessions as the sidebar lists them, each with its panes.
    groups: Vec<Group>,
    /// The pane with the keyboard: an index into `sessions`.
    active: usize,
    next_id: SessionId,
    /// The first terminal window: it restores the last sessions and saves
    /// its own.
    primary: bool,
    /// The last run's sessions, until the window opens them.
    restore: Option<(Vec<state::Group>, usize)>,
    /// A session's name while it is being edited.
    rename: Option<Rename>,
    sidebar_visible: bool,
    /// The settings panel, in place of the terminal while open.
    settings: Option<Settings>,
    /// Whether the mouse pointer is the hand shown over clickable things.
    pointer: bool,
    modifiers: ModifiersState,
    cursor: PhysicalPosition<f64>,
    /// What the resident last wrote about updates. Read at start, on focus,
    /// and while something is under way: no timer runs while idle.
    update: Status,
    /// Keep reading the status until then, so the answer to a request
    /// arrives even before the resident starts on it.
    update_watch_until: Option<Instant>,
    /// What a held left button is doing in the terminal.
    drag: Drag,
    /// The last left click in the terminal, to count double and triple clicks.
    last_click: Option<(Instant, Point, u8)>,
    /// The last cell a mouse report was sent for, so motion reports only
    /// go out when the cell changes.
    reported: Option<(usize, usize)>,
    /// The find bar, while open.
    find: Option<Find>,
    /// Ctrl+= and Ctrl+- change the font size for this window, in points.
    zoom: f32,
    /// A link under the mouse while Ctrl is held: its line, columns, target.
    link: Option<(u64, usize, usize, Target)>,
    /// Whether the window has keyboard focus: a session counts as seen only
    /// while it does.
    focused: bool,
    /// The prompt line of the command block under the mouse.
    hover_block: Option<u64>,
    /// Projects kept in the sidebar with no sessions open.
    pinned: Vec<PathBuf>,
    /// The project panel at the right of the terminal.
    panel_visible: bool,
    /// VS Code's `code.cmd`, when it is installed.
    editor: Option<PathBuf>,
    header_hover: Option<HeaderHit>,
    panel_hover: Option<PanelHit>,
}

/// Something that happened in a session you were not looking at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attention {
    Failed(i32),
    Done,
    Bell,
}

/// Commands shorter than this never notify: you were probably watching.
const NOTIFY_AFTER: Duration = Duration::from_secs(10);
/// Command timings kept per session, by prompt line.
const TIMINGS_KEPT: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drag {
    None,
    Selecting,
    /// A program that tracks the mouse gets the button's events.
    Reporting(Button),
}

/// Selected text, from where the press was to where the mouse is.
#[derive(Debug, Clone, Copy)]
struct Selection {
    anchor: Point,
    head: Point,
    /// A plain press selects nothing until the mouse moves.
    shown: bool,
}

impl Selection {
    fn ordered(&self) -> Option<(Point, Point)> {
        self.shown.then(|| {
            if self.anchor <= self.head {
                (self.anchor, self.head)
            } else {
                (self.head, self.anchor)
            }
        })
    }
}

struct Find {
    query: String,
    matches: Vec<Match>,
    current: Option<usize>,
}

/// Lines a wheel notch scrolls.
const WHEEL_LINES: usize = 3;
/// Clicks closer together than this count as a double or triple click.
const MULTI_CLICK: Duration = Duration::from_millis(450);

/// How often the update status is read while a check or download runs.
const UPDATE_POLL: Duration = Duration::from_millis(400);

/// The window and what draws into it; shared by every session.
struct View {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    atlas: Atlas,
    ui: Ui,
}

/// One shell: its PTY, what it printed, and what the bar and sidebar show.
struct Session {
    id: SessionId,
    term: Terminal,
    pty: Pty,
    input: File,
    context: Context,
    /// The shell's program name, such as `pwsh`: the session's title at the
    /// top of its project.
    shell: String,
    watcher: Option<Watcher>,
    /// Lines the view is scrolled up from the bottom.
    scroll: usize,
    selection: Option<Selection>,
    /// The project task this session was opened to run: its title.
    task: Option<String>,
    /// When the running command started, and its block's prompt line.
    running: Option<(Instant, u64)>,
    /// How long each finished command took, by its block's prompt line.
    timings: Vec<(u64, Duration)>,
    /// OSC 9;4 progress the program reports.
    progress: Option<u8>,
    attention: Option<Attention>,
    /// No notifications from this session.
    muted: bool,
    /// The shell when it is not the configured one; splits start it too.
    program: Option<Program>,
}

/// A session as the sidebar lists it: one shell, or several in panes.
struct Group {
    tree: Tree,
    /// The pane that has the keyboard while the session is shown.
    focus: SessionId,
    /// Only the focused pane is shown, over the whole area.
    zoomed: bool,
    /// Given with F2; without one the title follows the shell.
    name: Option<String>,
}

struct Rename {
    group: usize,
    text: String,
    /// The title when editing began: committing it unchanged keeps the
    /// title automatic.
    original: String,
}

/// Where everything goes in a window of a given size.
struct Layout {
    /// Width of the sidebar; 0 when it is hidden.
    sidebar: u32,
    /// Height of the session header above the terminal; 0 when it is off.
    header: u32,
    /// Width of the project panel at the right; 0 when it is closed.
    panel: u32,
    /// What the panes share.
    area: Rect,
}

/// The sidebar's width in logical pixels, as in T3 Code.
const SIDEBAR_WIDTH: f64 = 256.0;
/// The session header's height in logical pixels.
const HEADER_HEIGHT: f64 = 44.0;

impl App {
    pub fn new(proxy: EventLoopProxy<Event>, launch: Launch) -> Self {
        let (config, config_error) = match Config::default_path().map(|p| Config::load(&p)) {
            Some(Ok(config)) => (config, None),
            Some(Err(e)) => (Config::default(), Some(e.to_string())),
            None => (Config::default(), None),
        };
        let theme = Theme::new(&config.colors);
        let saved = state::load();
        let primary = aeon_launcher::sys::single_instance(if cfg!(debug_assertions) {
            "Local\\AeonWindowsDev"
        } else {
            "Local\\AeonWindows"
        });
        // Finding VS Code means walking PATH; keep that off the UI thread.
        let editor_proxy = proxy.clone();
        let _ = std::thread::Builder::new()
            .name("editor".into())
            .spawn(move || {
                let _ = editor_proxy.send_event(Event::Editor(find_on_path("code.cmd")));
            });
        Self {
            proxy,
            palette: Palette::new(&theme),
            theme,
            home: std::env::var_os("USERPROFILE").map(PathBuf::from),
            hover: None,
            config,
            config_error,
            launch: Some(launch),
            finder: None,
            finder_error: None,
            catalog: None,
            catalog_requested: None,
            view: None,
            sessions: Vec::new(),
            groups: Vec::new(),
            active: 0,
            next_id: 0,
            primary,
            restore: Some((saved.sessions, saved.active)),
            rename: None,
            sidebar_visible: saved.sidebar,
            pinned: saved.pinned,
            panel_visible: saved.panel,
            editor: None,
            header_hover: None,
            panel_hover: None,
            modifiers: ModifiersState::empty(),
            cursor: PhysicalPosition::new(-1.0, -1.0),
            settings: None,
            pointer: false,
            update: updater::read_status(),
            update_watch_until: None,
            drag: Drag::None,
            last_click: None,
            reported: None,
            find: None,
            zoom: 0.0,
            link: None,
            focused: true,
            hover_block: None,
        }
    }

    fn create_view(
        &self,
        event_loop: &ActiveEventLoop,
    ) -> Result<View, Box<dyn std::error::Error>> {
        let window_cfg = &self.config.window;
        let pad = u32::from(window_cfg.padding);
        let font = font::load(self.config.font.path.as_deref())?;
        // The window starts at the default scale and is resized on the first
        // ScaleFactorChanged, so size it from 96-DPI metrics.
        let atlas = Atlas::new(Rc::new(font), points_to_px(self.config.font.size, 1.0));
        let header = if self.config.bar.enabled {
            HEADER_HEIGHT as u32
        } else {
            0
        };
        let sidebar = if self.sidebar_visible {
            SIDEBAR_WIDTH as u32
        } else {
            0
        };
        let inner = winit::dpi::LogicalSize::new(
            sidebar + u32::from(window_cfg.columns) * atlas.cell_width + 2 * pad,
            u32::from(window_cfg.rows) * atlas.cell_height + 2 * pad + header,
        );

        let mut title = format!("Aeon {}", crate::VERSION);
        if let Some(error) = &self.config_error {
            title.push_str(&format!(" (config ignored: {error})"));
        }
        let window = Rc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title(title)
                    .with_window_icon(Icon::from_resource(1, None).ok())
                    .with_theme(Some(window_theme(&self.theme)))
                    .with_inner_size(inner),
            )?,
        );
        let atlas = atlas.with_px(self.font_px(window.scale_factor()));
        let context = softbuffer::Context::new(window.clone())?;
        let surface = softbuffer::Surface::new(&context, window.clone())?;
        Ok(View {
            ui: Ui::new(window.scale_factor()),
            window,
            surface,
            atlas,
        })
    }

    /// Starts a shell in `cwd` as a new session and shows it.
    fn open_session(
        &mut self,
        cwd: Option<PathBuf>,
        run: Option<String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.open_with(cwd, run, None)
    }

    /// Like `open_session`, with `program` in place of the configured shell.
    fn open_with(
        &mut self,
        cwd: Option<PathBuf>,
        run: Option<String>,
        program: Option<Program>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let view = self.view.as_ref().ok_or("no window")?;
        let (_, rows, cols) = self.grid_in(view, self.layout(view).area);
        let id = self.next_id;
        self.next_id += 1;
        self.spawn_pane(id, cwd, run, program, (rows, cols))?;
        self.groups.push(Group {
            tree: Tree::Pane(id),
            focus: id,
            zoomed: false,
            name: None,
        });
        self.focus_pane(self.sessions.len() - 1);
        Ok(())
    }

    /// Starts a shell sized `rows` by `cols` and adds it to `sessions`.
    fn spawn_pane(
        &mut self,
        id: SessionId,
        cwd: Option<PathBuf>,
        run: Option<String>,
        program: Option<Program>,
        (rows, cols): (usize, usize),
    ) -> Result<(), Box<dyn std::error::Error>> {
        let shell = self.shell_command(cwd, run.as_deref(), program.as_ref());
        let shell_name = std::path::Path::new(&shell.program)
            .file_stem()
            .map_or_else(
                || "shell".to_owned(),
                |n| n.to_string_lossy().to_lowercase(),
            );
        let mut pty = Pty::spawn(&shell, pty_size(rows, cols))?;
        let output = pty.take_reader().ok_or("pty reader already taken")?;
        let input = pty.take_writer().ok_or("pty writer already taken")?;
        spawn_reader(id, output, self.proxy.clone());
        // ConPTY keeps the output pipe open after the shell exits, so `exit`
        // is noticed on the process itself.
        let waiter = pty.exit_waiter()?;
        let proxy = self.proxy.clone();
        std::thread::Builder::new()
            .name(format!("pty-exit-{id}"))
            .spawn(move || {
                let _ = waiter.wait();
                let _ = proxy.send_event(Event::Exited(id));
            })?;

        // The sidebar shows every session's branch, so each one is watched
        // whenever either the bar or the sidebar can show it.
        let watcher = (self.config.bar.enabled || self.sidebar_visible).then(|| {
            let proxy = self.proxy.clone();
            Watcher::spawn(pty.pid(), shell.cwd.clone(), move |context| {
                let _ = proxy.send_event(Event::Context(id, context));
            })
        });

        self.sessions.push(Session {
            id,
            term: Terminal::with_scrollback(rows, cols, self.config.terminal.scrollback),
            pty,
            input,
            context: Context {
                cwd: shell.cwd,
                ..Context::default()
            },
            shell: shell_name,
            watcher,
            scroll: 0,
            selection: None,
            task: None,
            running: None,
            timings: Vec::new(),
            progress: None,
            attention: None,
            muted: false,
            program,
        });
        Ok(())
    }

    /// Halves the focused pane, starting a shell in the new half where the
    /// focused one is.
    fn split(&mut self, split: Split) {
        let (Some(view), Some(g)) = (self.view.as_ref(), self.active_group()) else {
            return;
        };
        let id = self.next_id;
        let mut tree = self.groups[g].tree.clone();
        tree.split(self.groups[g].focus, id, split);
        let (mut rects, mut dividers) = (Vec::new(), Vec::new());
        tree.layout(
            self.layout(view).area,
            divider(view),
            &mut rects,
            &mut dividers,
        );
        let Some(&(_, rect)) = rects.iter().find(|(p, _)| *p == id) else {
            return;
        };
        let (_, rows, cols) = self.grid_in(view, rect);
        let (cwd, muted, program) = self.sessions.get(self.active).map_or_else(
            || (None, false, None),
            |s| (s.context.cwd.clone(), s.muted, s.program.clone()),
        );
        self.next_id += 1;
        if let Err(e) = self.spawn_pane(id, cwd, None, program, (rows, cols)) {
            eprintln!("aeon: could not split: {e}");
            return;
        }
        if let Some(session) = self.sessions.last_mut() {
            session.muted = muted;
        }
        let group = &mut self.groups[g];
        group.tree = tree;
        group.zoomed = false;
        self.focus_pane(self.sessions.len() - 1);
        self.resize();
    }

    /// Shows only the focused pane, or all of them again.
    fn toggle_zoom(&mut self) {
        let Some(g) = self.active_group() else {
            return;
        };
        let group = &mut self.groups[g];
        if matches!(group.tree, Tree::Split { .. }) || group.zoomed {
            group.zoomed = !group.zoomed;
            self.resize();
        }
    }

    fn shell_command(
        &self,
        cwd: Option<PathBuf>,
        run: Option<&str>,
        program: Option<&Program>,
    ) -> aeon_pty::Command {
        let shell = &self.config.shell;
        let mut command = match (program, &shell.program) {
            (Some(p), _) => aeon_pty::Command::new(&p.program).args(&p.args),
            (None, Some(program)) => aeon_pty::Command::new(program).args(&shell.args),
            (None, None) => aeon_pty::default_shell(),
        };
        if shell.integration {
            command = aeon_pty::integrate(command, run);
        }
        command.cwd = cwd.or_else(|| shell.cwd.clone()).or_else(start_dir);
        command
            .env("TERM_PROGRAM", "aeon")
            .env("TERM_PROGRAM_VERSION", crate::VERSION)
            .env("COLORTERM", "truecolor")
    }

    /// A new session starts where the active one is.
    fn new_session(&mut self) {
        let cwd = self
            .sessions
            .get(self.active)
            .and_then(|s| s.context.cwd.clone());
        if let Err(e) = self.open_session(cwd, None) {
            eprintln!("aeon: could not open a session: {e}");
        }
    }

    /// Closes pane `index`. Its sibling takes its space; a session's last
    /// pane closes the session, and the last session closes the window.
    fn close_pane(&mut self, index: usize, event_loop: &ActiveEventLoop) {
        let (Some(id), Some(g)) = (self.sessions.get(index).map(|s| s.id), self.group_of(index))
        else {
            return;
        };
        let active = self.sessions.get(self.active).map(|s| s.id);
        let group = &mut self.groups[g];
        let kept = group.tree.remove(id);
        if kept {
            if group.focus == id {
                group.focus = group.tree.panes()[0];
            }
            group.zoomed = false;
        } else {
            self.groups.remove(g);
            self.rename = None;
        }
        // Dropping the session closes its pseudo console, which ends the shell.
        self.sessions.remove(index);
        if self.groups.is_empty() {
            self.quit(event_loop);
            return;
        }
        let next = match active {
            Some(a) if a != id => a,
            _ => self.groups[g.min(self.groups.len() - 1)].focus,
        };
        self.active = self.session_index(next).unwrap_or(0);
        self.update_title();
        self.resize();
        self.mark_seen();
    }

    /// Closes the focused pane, asking first if a command is running in it.
    fn close_focused(&mut self, event_loop: &ActiveEventLoop) {
        if self.confirm_close(&[self.active]) {
            self.close_pane(self.active, event_loop);
        }
    }

    /// What pane `index` is running, when a command is under way.
    fn running_command(&self, index: usize) -> Option<String> {
        let session = self.sessions.get(index)?;
        let (_, prompt) = session.running?;
        Some(
            session
                .term
                .blocks()
                .find(|b| b.prompt == prompt)
                .map(|b| b.command.clone())
                .unwrap_or_default(),
        )
    }

    /// Asks before closing panes with a command under way. Shells that
    /// report no commands (integration off) never ask.
    fn confirm_close(&self, panes: &[usize]) -> bool {
        let running: Vec<String> = panes
            .iter()
            .filter_map(|&i| self.running_command(i))
            .collect();
        let text = match running.as_slice() {
            [] => return true,
            [one] if !one.is_empty() => {
                format!("\u{201c}{one}\u{201d} is still running. Close it anyway?")
            }
            [_] => "A command is still running. Close it anyway?".to_owned(),
            many => format!(
                "{} commands are still running. Close them anyway?",
                many.len()
            ),
        };
        let owner = self.view.as_ref().map_or(0, |v| hwnd(&v.window));
        crate::sys::confirm(owner, "Aeon", &text)
    }

    /// Saves what the next start restores, then closes the window.
    fn quit(&mut self, event_loop: &ActiveEventLoop) {
        self.save_state();
        event_loop.exit();
    }

    /// Gives pane `index` the keyboard, showing its session.
    fn focus_pane(&mut self, index: usize) {
        let Some(id) = self.sessions.get(index).map(|s| s.id) else {
            return;
        };
        if let Some(g) = self.group_of(index) {
            self.groups[g].focus = id;
        }
        if index != self.active {
            self.active = index;
            self.drag = Drag::None;
            self.link = None;
            self.hover_block = None;
            // Matches belong to the pane they were found in.
            self.find = None;
        }
        self.update_title();
        self.request_redraw();
        self.mark_seen();
    }

    /// Shows session `g`, as the sidebar numbers them, at the pane it last
    /// had focused.
    fn select(&mut self, g: usize) {
        if let Some(index) = self
            .groups
            .get(g)
            .and_then(|group| self.session_index(group.focus))
        {
            self.focus_pane(index);
        }
    }

    /// The session pane `index` belongs to.
    fn group_of(&self, index: usize) -> Option<usize> {
        let id = self.sessions.get(index)?.id;
        self.groups.iter().position(|g| g.tree.contains(id))
    }

    fn active_group(&self) -> Option<usize> {
        self.group_of(self.active)
    }

    /// The pane a session shows in the sidebar and header: its focused one.
    fn lead(&self, group: &Group) -> Option<&Session> {
        self.session_index(group.focus).map(|i| &self.sessions[i])
    }

    /// Whether pane `index` is on screen.
    fn is_shown(&self, index: usize) -> bool {
        let (Some(session), Some(g)) = (self.sessions.get(index), self.active_group()) else {
            return false;
        };
        let group = &self.groups[g];
        group.tree.contains(session.id) && (!group.zoomed || group.focus == session.id)
    }

    /// The panes of session `g` on screen and their rectangles, with the
    /// dividers between them.
    fn pane_rects(&self, view: &View, g: usize) -> (Vec<(SessionId, Rect)>, Vec<Rect>) {
        let area = self.layout(view).area;
        let group = &self.groups[g];
        if group.zoomed {
            return (vec![(group.focus, area)], Vec::new());
        }
        let (mut rects, mut dividers) = (Vec::new(), Vec::new());
        group
            .tree
            .layout(area, divider(view), &mut rects, &mut dividers);
        (rects, dividers)
    }

    /// The active session's panes on screen, by index into `sessions`.
    fn shown_panes(&self) -> Vec<(usize, Rect)> {
        let (Some(view), Some(g)) = (self.view.as_ref(), self.active_group()) else {
            return Vec::new();
        };
        self.pane_rects(view, g)
            .0
            .into_iter()
            .filter_map(|(id, rect)| Some((self.session_index(id)?, rect)))
            .collect()
    }

    /// Where a pane's cells start in `rect`, and how many rows and columns
    /// fit.
    fn grid_in(&self, view: &View, rect: Rect) -> ((u32, u32), usize, usize) {
        let scale = view.window.scale_factor();
        let pad = (f64::from(self.config.window.padding) * scale).round() as u32;
        let w = rect.w.saturating_sub(2 * pad);
        let h = rect.h.saturating_sub(2 * pad);
        (
            (rect.x + pad, rect.y + pad),
            (h / view.atlas.cell_height).max(1) as usize,
            (w / view.atlas.cell_width).max(1) as usize,
        )
    }

    /// The active session has been looked at: its marks clear.
    fn mark_seen(&mut self) {
        let Some(g) = self.active_group().filter(|_| self.focused) else {
            return;
        };
        let mut cleared = false;
        for id in self.groups[g].tree.panes() {
            if let Some(i) = self.session_index(id) {
                cleared |= self.sessions[i].attention.take().is_some();
            }
        }
        if cleared {
            self.request_redraw();
        }
    }

    /// Reacts to what happened in session `index`: timing, status, and a
    /// notification when a long command ends out of sight.
    fn shell_events(&mut self, index: usize) {
        let visible = self.is_shown(index) && self.focused && self.settings.is_none();
        let project = {
            let group = self.group_of(index);
            let projects = self.projects();
            projects
                .iter()
                .find(|p| group.is_some_and(|g| p.sessions.contains(&g)))
                .map(|p| p.name.clone())
                .unwrap_or_default()
        };
        let Some(session) = self.sessions.get_mut(index) else {
            return;
        };
        let mut notify = None;
        for event in session.term.take_events() {
            match event {
                TermEvent::CommandStarted { command } if !command.is_empty() => {
                    let prompt = session.term.blocks().last().map_or(0, |b| b.prompt);
                    session.running = Some((Instant::now(), prompt));
                    session.progress = None;
                }
                TermEvent::CommandStarted { .. } => {}
                TermEvent::CommandFinished { command, exit } => {
                    let Some((start, prompt)) = session.running.take() else {
                        continue;
                    };
                    let took = start.elapsed();
                    if session.timings.len() >= TIMINGS_KEPT {
                        session.timings.remove(0);
                    }
                    session.timings.push((prompt, took));
                    session.progress = None;
                    let failed = exit.filter(|code| *code != 0);
                    if !visible {
                        if let Some(code) = failed {
                            session.attention = Some(Attention::Failed(code));
                        } else if took >= NOTIFY_AFTER {
                            session.attention = Some(Attention::Done);
                        }
                    }
                    if !visible && !session.muted && took >= NOTIFY_AFTER {
                        let place = if project.is_empty() {
                            String::new()
                        } else {
                            format!(" in {project}")
                        };
                        let took = chrome::format_duration(took);
                        notify = Some(match failed {
                            Some(code) => (
                                format!("{command} failed{place}"),
                                format!("Exit {code} after {took}."),
                            ),
                            None => (
                                format!("{command} finished{place}"),
                                format!("Done in {took}."),
                            ),
                        });
                    }
                }
                TermEvent::Bell if !visible && session.attention.is_none() => {
                    session.attention = Some(Attention::Bell);
                }
                TermEvent::Bell => {}
                TermEvent::Progress(p) => session.progress = p,
            }
        }
        if let Some((title, body)) = notify {
            sys::notify_through_resident(&title, &body);
            if let Some(view) = &self.view {
                view.window
                    .request_user_attention(Some(winit::window::UserAttentionType::Informational));
            }
        }
    }

    /// What the sidebar shows for a session: the first of its panes that
    /// needs a look, else what its panes are doing.
    fn group_state(&self, group: &Group) -> (RowState, bool) {
        let panes: Vec<&Session> = group
            .tree
            .panes()
            .into_iter()
            .filter_map(|id| self.session_index(id).map(|i| &self.sessions[i]))
            .collect();
        if let Some(attention) = panes.iter().find_map(|s| s.attention) {
            let state = match attention {
                Attention::Failed(code) => RowState::Failed(code),
                Attention::Done => RowState::Done,
                Attention::Bell => RowState::Bell,
            };
            return (state, true);
        }
        let state = if let Some(p) = panes.iter().find_map(|s| s.progress) {
            RowState::Progress(p)
        } else if let Some(port) = panes
            .iter()
            .find_map(|s| s.context.servers.first().map(|server| server.port))
        {
            RowState::Server(port)
        } else if panes.iter().any(|s| s.running.is_some()) {
            RowState::Running
        } else {
            RowState::Idle
        };
        (state, false)
    }

    /// Command blocks with their prompt on screen, for pane `index`.
    fn visible_blocks(&self, index: usize) -> (Vec<BlockMark>, Vec<(u64, u64)>) {
        let Some(session) = self.sessions.get(index) else {
            return (Vec::new(), Vec::new());
        };
        let term = &session.term;
        if term.modes().alt_screen {
            return (Vec::new(), Vec::new());
        }
        let first = term.screen_line().saturating_sub(session.scroll as u64);
        let rows = term.grid().rows() as u64;
        let mut marks = Vec::new();
        let mut tints = Vec::new();
        for block in term.blocks() {
            if block.command.is_empty() || block.output.is_none() {
                continue;
            }
            let took = session
                .timings
                .iter()
                .rev()
                .find(|(p, _)| *p == block.prompt)
                .map(|(_, d)| *d);
            let status = match (block.end, block.exit) {
                (None, _) => BlockStatus::Running,
                (Some(_), Some(0)) => BlockStatus::Passed(took),
                (Some(end), Some(code)) => {
                    tints.push((block.prompt, end));
                    BlockStatus::Failed(code, took)
                }
                (Some(_), None) => BlockStatus::Ended,
            };
            if (first..first + rows).contains(&block.prompt) {
                marks.push(BlockMark {
                    row: (block.prompt - first) as u32,
                    prompt: block.prompt,
                    status,
                    hovered: index == self.active && self.hover_block == Some(block.prompt),
                });
            }
        }
        (marks, tints)
    }

    fn term_area(&self, index: usize, rect: Rect) -> Option<TermArea> {
        let view = self.view.as_ref()?;
        let ((left, top), _, _) = self.grid_in(view, rect);
        // The chip covers the last columns exactly, so no glyph peeks out.
        let cols = self.sessions.get(index)?.term.grid().cols();
        Some(TermArea {
            left: rect.x,
            right: left + cols as u32 * view.atlas.cell_width,
            top,
            cell_height: view.atlas.cell_height,
        })
    }

    /// The focused pane's terminal area, for its block buttons.
    fn active_term_area(&self) -> Option<TermArea> {
        let (_, rect) = self
            .shown_panes()
            .into_iter()
            .find(|(i, _)| *i == self.active)?;
        self.term_area(self.active, rect)
    }

    /// Runs a block button: copy what the command printed, or run it again.
    fn block_action(&mut self, prompt: u64, action: BlockAction) {
        let Some(session) = self.sessions.get_mut(self.active) else {
            return;
        };
        let Some(block) = session.term.blocks().find(|b| b.prompt == prompt).cloned() else {
            return;
        };
        match action {
            BlockAction::CopyOutput => {
                let (Some(output), Some(end)) = (block.output, block.end) else {
                    return;
                };
                let text = if end > output {
                    let cols = session.term.grid().cols();
                    session.term.text(
                        Point {
                            line: output,
                            col: 0,
                        },
                        Point {
                            line: end - 1,
                            col: cols.saturating_sub(1),
                        },
                    )
                } else {
                    String::new()
                };
                let owner = self.view.as_ref().map_or(0, |v| hwnd(&v.window));
                sys::set_clipboard(owner, text.trim_end());
            }
            BlockAction::Rerun => {
                session.scroll = 0;
                session.selection = None;
                let mut bytes = block.command.into_bytes();
                bytes.push(b'\r');
                let _ = session.input.write_all(&bytes);
            }
        }
        self.request_redraw();
    }

    /// Scrolls so the previous (`-1`) or next (`1`) command's prompt is at
    /// the top of the view.
    fn jump_prompt(&mut self, step: i32) {
        let Some(session) = self.sessions.get_mut(self.active) else {
            return;
        };
        let screen = session.term.screen_line();
        let top = screen.saturating_sub(session.scroll as u64);
        let prompts: Vec<u64> = session.term.blocks().map(|b| b.prompt).collect();
        let target = if step < 0 {
            prompts.iter().rev().find(|&&p| p < top).copied()
        } else {
            prompts.iter().find(|&&p| p > top).copied()
        };
        let scroll = match target {
            Some(line) => screen
                .saturating_sub(line)
                .min(session.term.scrollback_len() as u64) as usize,
            None if step > 0 => 0,
            None => return,
        };
        if scroll != session.scroll {
            session.scroll = scroll;
            self.request_redraw();
        }
    }

    fn toggle_sidebar(&mut self) {
        self.sidebar_visible = !self.sidebar_visible;
        self.save_state();
        self.resize();
    }

    fn save_state(&self) {
        // Only the first window's sessions are restored; another window
        // keeps the ones already saved.
        let sessions = self
            .primary
            .then(|| (self.saved_groups(), self.active_group().unwrap_or(0)));
        state::save(
            state::Saved {
                sidebar: self.sidebar_visible,
                panel: self.panel_visible,
                pinned: self.pinned.clone(),
                sessions: Vec::new(),
                active: 0,
            },
            sessions,
        );
    }

    fn toggle_panel(&mut self) {
        self.panel_visible = !self.panel_visible;
        self.save_state();
        self.resize();
    }

    /// The project the active session belongs to.
    fn active_project(&self) -> Option<Project> {
        let g = self.active_group()?;
        self.projects()
            .into_iter()
            .find(|p| p.sessions.contains(&g))
    }

    /// Every pane of a project's sessions.
    fn project_panes(&self, project: &Project) -> Vec<usize> {
        project
            .sessions
            .iter()
            .flat_map(|&g| self.groups[g].tree.panes())
            .filter_map(|id| self.session_index(id))
            .collect()
    }

    /// What the project panel shows for the active session's project.
    fn panel_data(
        &self,
    ) -> (
        Vec<aeon_context::Task>,
        Vec<bool>,
        Vec<PanelServer>,
        Option<aeon_context::Git>,
    ) {
        let Some(project) = self.active_project() else {
            return Default::default();
        };
        let Some(session) = self.sessions.get(self.active) else {
            return Default::default();
        };
        let tasks = session.context.tasks.clone();
        let panes = self.project_panes(&project);
        let running = tasks
            .iter()
            .map(|t| {
                panes.iter().any(|&i| {
                    self.sessions[i].task.as_deref() == Some(t.name.as_str())
                        && self.sessions[i].running.is_some()
                })
            })
            .collect();
        let home = self.home.as_deref();
        let servers = panes
            .iter()
            .flat_map(|&i| {
                let s = &self.sessions[i];
                let title = s
                    .task
                    .clone()
                    .unwrap_or_else(|| chrome::session_title(&s.context, &s.shell, &project, home));
                s.context.servers.iter().map(move |server| PanelServer {
                    name: server.name.clone(),
                    port: server.port,
                    session: title.clone(),
                })
            })
            .collect();
        (tasks, running, servers, session.context.git.clone())
    }

    /// Runs project task `index` in a new session at the project's top.
    fn run_task(&mut self, index: usize) {
        let Some(task) = self
            .sessions
            .get(self.active)
            .and_then(|s| s.context.tasks.get(index))
            .cloned()
        else {
            return;
        };
        let cwd = self.active_project().and_then(|p| p.path);
        if let Err(e) = self.open_session(cwd, Some(task.command.clone())) {
            eprintln!("aeon: could not run {}: {e}", task.name);
            return;
        }
        if let Some(session) = self.sessions.last_mut() {
            session.task = Some(task.name);
        }
    }

    /// Interrupts a session with Ctrl+C: the program stops and the session
    /// keeps its output.
    fn interrupt(&mut self, index: usize) {
        if let Some(session) = self.sessions.get_mut(index) {
            let _ = session.input.write_all(b"\x03");
        }
    }

    fn panel_action(&mut self, hit: PanelHit) {
        match hit {
            PanelHit::Run(i) => self.run_task(i),
            PanelHit::Stop(i) => {
                let name = self
                    .sessions
                    .get(self.active)
                    .and_then(|s| s.context.tasks.get(i))
                    .map(|t| t.name.clone());
                if let (Some(name), Some(project)) = (name, self.active_project()) {
                    let target = self.project_panes(&project).into_iter().find(|&s| {
                        self.sessions[s].task.as_deref() == Some(name.as_str())
                            && self.sessions[s].running.is_some()
                    });
                    if let Some(target) = target {
                        self.interrupt(target);
                    }
                }
            }
            PanelHit::OpenServer(i) | PanelHit::StopServer(i) => {
                let Some(project) = self.active_project() else {
                    return;
                };
                let owners: Vec<(usize, u16)> = self
                    .project_panes(&project)
                    .into_iter()
                    .flat_map(|s| {
                        self.sessions[s]
                            .context
                            .servers
                            .iter()
                            .map(move |server| (s, server.port))
                    })
                    .collect();
                let Some(&(owner, port)) = owners.get(i) else {
                    return;
                };
                if matches!(hit, PanelHit::OpenServer(_)) {
                    sys::open_url(&format!("http://localhost:{port}/"));
                } else {
                    self.interrupt(owner);
                }
            }
        }
        self.request_redraw();
    }

    fn open_editor(&self) {
        let (Some(code), Some(project)) = (&self.editor, self.active_project()) else {
            return;
        };
        let folder = project.path.or_else(|| {
            self.sessions
                .get(self.active)
                .and_then(|s| s.context.cwd.clone())
        });
        if let Some(folder) = folder {
            let _ = hidden(std::process::Command::new(code).arg(folder)).spawn();
        }
    }

    /// The project panel's layout for the window's current size.
    fn panel_layout(&mut self, data: &PanelData<'_>) -> Option<Panel> {
        let layout = self.layout(self.view.as_ref()?);
        if layout.panel == 0 {
            return None;
        }
        let view = self.view.as_mut()?;
        let size = view.window.inner_size();
        Some(Panel::new(
            &mut view.ui,
            size.width - layout.panel,
            layout.panel,
            layout.header,
            size.height,
            data,
        ))
    }

    /// What the header or panel has under (`x`, `y`).
    fn right_hit(&mut self, x: f64, y: f64) -> (Option<HeaderHit>, Option<PanelHit>) {
        if self.settings.is_some() {
            return (None, None);
        }
        let Some(view) = self.view.as_ref() else {
            return (None, None);
        };
        let layout = self.layout(view);
        if x < f64::from(layout.sidebar) {
            return (None, None);
        }
        if layout.header > 0 && y < f64::from(layout.header) {
            let editor = self.editor.is_some();
            let Some(view) = self.view.as_mut() else {
                return (None, None);
            };
            let width = view.window.inner_size().width;
            let hit = chrome::header_buttons(&mut view.ui, width, editor)
                .into_iter()
                .find(|(r, _)| r.contains(x, y))
                .map(|(_, h)| h);
            return (hit, None);
        }
        if layout.panel == 0 {
            return (None, None);
        }
        let (tasks, running, servers, git) = self.panel_data();
        let data = PanelData {
            tasks: &tasks,
            running: &running,
            servers: &servers,
            git: git.as_ref(),
        };
        let hit = self.panel_layout(&data).and_then(|panel| panel.hit(x, y));
        (None, hit)
    }

    fn update_title(&self) {
        let (Some(view), Some(session)) = (&self.view, self.sessions.get(self.active)) else {
            return;
        };
        let place = session
            .context
            .cwd
            .as_deref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        let mut title = match place {
            Some(place) => format!("{place} \u{2014} Aeon"),
            None => format!("Aeon {}", crate::VERSION),
        };
        if let Some(error) = &self.config_error {
            title.push_str(&format!(" (config ignored: {error})"));
        }
        view.window.set_title(&title);
    }

    fn request_redraw(&self) {
        if let Some(view) = &self.view {
            view.window.request_redraw();
        }
    }

    fn layout(&self, view: &View) -> Layout {
        let size = view.window.inner_size();
        let sidebar = if self.sidebar_visible {
            view.ui.px(SIDEBAR_WIDTH).min(size.width / 2)
        } else {
            0
        };
        let header = if self.config.bar.enabled {
            view.ui.px(HEADER_HEIGHT)
        } else {
            0
        };
        // The panel needs room for itself and a usable terminal beside it.
        let panel_width = view.ui.px(chrome::PANEL_WIDTH);
        let panel = if self.panel_visible
            && self.settings.is_none()
            && size.width >= sidebar + panel_width + view.ui.px(360.0)
        {
            panel_width
        } else {
            0
        };
        Layout {
            sidebar,
            header,
            panel,
            area: Rect {
                x: sidebar,
                y: header,
                w: size.width.saturating_sub(sidebar + panel),
                h: size.height.saturating_sub(header),
            },
        }
    }

    /// Sessions grouped into projects, as the sidebar shows them. A
    /// session belongs where its focused pane is.
    fn projects(&self) -> Vec<Project> {
        let contexts: Vec<&Context> = self
            .groups
            .iter()
            .map(|g| {
                &self
                    .lead(g)
                    .expect("a session's focus is one of its panes")
                    .context
            })
            .collect();
        chrome::projects(&contexts, self.home.as_deref(), &self.pinned)
    }

    /// Moves `step` sessions along the sidebar's order, wrapping around.
    fn step(&mut self, step: isize) {
        let order = chrome::display_order(&self.projects());
        let Some(g) = self.active_group() else {
            return;
        };
        let Some(at) = order.iter().position(|&i| i == g) else {
            return;
        };
        let n = order.len() as isize;
        self.select(order[(at as isize + step).rem_euclid(n) as usize]);
    }

    /// What the sidebar has under (`x`, `y`), if anything.
    fn sidebar_hit(&mut self, x: f64, y: f64) -> Option<Hit> {
        let layout = self.layout(self.view.as_ref()?);
        if x >= f64::from(layout.sidebar) {
            return None;
        }
        let projects = self.projects();
        let pill = self.pill();
        let view = self.view.as_mut()?;
        let height = view.window.inner_size().height;
        Sidebar::new(
            &mut view.ui,
            layout.sidebar,
            height,
            &projects,
            pill.as_ref(),
        )
        .hit(x, y)
    }

    /// Fits every pane's grid and PTY to its place in the window.
    fn resize(&mut self) {
        let Some(view) = &self.view else {
            return;
        };
        let mut sizes = Vec::new();
        for g in 0..self.groups.len() {
            for (id, rect) in self.pane_rects(view, g).0 {
                let (_, rows, cols) = self.grid_in(view, rect);
                sizes.push((id, rows, cols));
            }
        }
        for (id, rows, cols) in sizes {
            let Some(session) = self.session_index(id).map(|i| &mut self.sessions[i]) else {
                continue;
            };
            if (rows, cols) != (session.term.grid().rows(), session.term.grid().cols()) {
                session.term.resize(rows, cols);
                let _ = session.pty.resize(pty_size(rows, cols));
            }
        }
        self.request_redraw();
    }

    /// Session `g`'s title: its name, its task, or where its focused pane is.
    fn group_title(&self, g: usize, projects: &[Project]) -> String {
        let group = &self.groups[g];
        if let Some(name) = &group.name {
            return name.clone();
        }
        let Some(s) = self.lead(group) else {
            return String::new();
        };
        if let Some(task) = &s.task {
            return task.clone();
        }
        projects
            .iter()
            .find(|p| p.sessions.contains(&g))
            .map_or_else(
                || s.shell.clone(),
                |p| chrome::session_title(&s.context, &s.shell, p, self.home.as_deref()),
            )
    }

    fn redraw(&mut self) {
        let Some(view) = self.view.as_ref() else {
            return;
        };
        let layout = self.layout(view);
        let scale = view.window.scale_factor();
        let projects = self.projects();
        let pill = self.pill();
        let active_group = self.active_group();
        let dividers = active_group.map_or_else(Vec::new, |g| self.pane_rects(view, g).1);
        // Everything each pane on screen needs, worked out before the
        // window is borrowed for drawing.
        let panes: Vec<_> = self
            .shown_panes()
            .into_iter()
            .map(|(i, rect)| {
                let (marks, tints) = self.visible_blocks(i);
                let (origin, _, _) = self.grid_in(view, rect);
                (i, rect, origin, marks, tints, self.term_area(i, rect))
            })
            .collect();
        let (tasks, running, servers, git) = self.panel_data();
        let panel_data = PanelData {
            tasks: &tasks,
            running: &running,
            servers: &servers,
            git: git.as_ref(),
        };
        let panel = self.panel_layout(&panel_data);
        let editor = self.editor.is_some();
        let finder_rect = self.finder_rect();
        let header_title = active_group.map(|g| self.group_title(g, &projects));
        let rows: Vec<SessionRow> = if layout.sidebar > 0 {
            self.groups
                .iter()
                .enumerate()
                .map(|(g, group)| {
                    let (state, unseen) = self.group_state(group);
                    let editing = self.rename.as_ref().filter(|r| r.group == g);
                    SessionRow {
                        title: editing
                            .map_or_else(|| self.group_title(g, &projects), |r| r.text.clone()),
                        state,
                        unseen,
                        muted: self.lead(group).is_some_and(|s| s.muted),
                        panes: group.tree.panes().len(),
                        editing: editing.is_some(),
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let size = view.window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        if view.surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buffer) = view.surface.buffer_mut() else {
            return;
        };
        let mut frame = Frame {
            pixels: &mut buffer,
            width: w.get(),
            height: h.get(),
        };
        let Some(session) = self.sessions.get(self.active) else {
            return;
        };
        if let Some(panel) = &self.settings {
            frame.pixels.fill(self.palette.canvas);
            let geo = settings::Geometry::new(layout.sidebar, frame.width, scale);
            panel.draw(&mut frame, &mut view.ui, &self.palette, &geo);
        } else {
            let tint = render::blend(self.theme.bg, self.theme.ansi[1], 22);
            for (i, rect, origin, marks, tints, area) in &panes {
                let pane = &self.sessions[*i];
                let overlay = if *i == self.active {
                    Overlay {
                        scroll: pane.scroll,
                        selection: pane.selection.and_then(|s| s.ordered()),
                        matches: self.find.as_ref().map_or(&[], |f| &f.matches[..]),
                        current: self.find.as_ref().and_then(|f| f.current),
                        link: self.link.as_ref().map(|(line, a, b, _)| (*line, *a, *b)),
                        tints,
                        tint,
                        hollow_cursor: !self.focused,
                    }
                } else {
                    Overlay {
                        scroll: pane.scroll,
                        selection: pane.selection.and_then(|s| s.ordered()),
                        tints,
                        tint,
                        hollow_cursor: true,
                        ..Overlay::default()
                    }
                };
                render::draw(
                    &mut frame,
                    &pane.term,
                    &mut view.atlas,
                    &self.theme,
                    *rect,
                    *origin,
                    self.config.cursor.style,
                    &overlay,
                );
                if let Some(area) = area {
                    chrome::draw_blocks(&mut frame, &mut view.ui, &self.palette, area, marks);
                }
            }
            for d in &dividers {
                render::fill_rect(&mut frame, d.x, d.y, d.w, d.h, self.palette.border);
            }
            // With more than one pane, a line over the focused one says
            // where typing goes.
            if panes.len() > 1
                && let Some((_, rect, ..)) = panes.iter().find(|(i, ..)| *i == self.active)
            {
                let thick = view.ui.px(2.0).max(1);
                render::fill_rect(
                    &mut frame,
                    rect.x,
                    rect.y,
                    rect.w,
                    thick,
                    self.palette.accent,
                );
            }
            if layout.header > 0
                && let (Some(g), Some(title)) = (active_group, &header_title)
                && let Some(project) = projects.iter().find(|p| p.sessions.contains(&g))
            {
                let buttons = chrome::header_buttons(&mut view.ui, frame.width, editor);
                chrome::draw_header(
                    &mut frame,
                    &mut view.ui,
                    &self.palette,
                    layout.sidebar,
                    project,
                    title,
                    &session.context,
                    &buttons,
                    self.header_hover,
                    layout.panel > 0,
                );
            }
            if let Some(panel) = &panel {
                chrome::draw_panel(
                    &mut frame,
                    &mut view.ui,
                    &self.palette,
                    panel,
                    &panel_data,
                    self.panel_hover,
                );
            }
            if let Some(find) = &self.find {
                let count = match (find.current, find.matches.len()) {
                    (_, 0) if find.query.is_empty() => String::new(),
                    (_, 0) => "No results".to_owned(),
                    (Some(i), n) => format!("{} of {n}", i + 1),
                    (None, n) => format!("{n} found"),
                };
                chrome::draw_find(
                    &mut frame,
                    &mut view.ui,
                    &self.palette,
                    layout.header,
                    &find.query,
                    &count,
                );
            }
        }
        if layout.sidebar > 0 {
            let sidebar = Sidebar::new(
                &mut view.ui,
                layout.sidebar,
                frame.height,
                &projects,
                pill.as_ref(),
            );
            chrome::draw_sidebar(
                &mut frame,
                &mut view.ui,
                &self.palette,
                &sidebar,
                &SidebarState {
                    projects: &projects,
                    sessions: &rows,
                    active: active_group.unwrap_or(0),
                    settings_open: self.settings.is_some(),
                    hover: self.hover,
                    pill: pill.as_ref(),
                    new_keys: &self.config.keys.new_session,
                },
            );
        }
        if let (Some(rect), Some(launcher)) = (finder_rect, &self.finder) {
            finder::draw(
                &mut frame,
                &mut view.ui,
                &self.palette,
                rect,
                &Content {
                    launcher,
                    error: self.finder_error.as_deref(),
                    icons: None,
                    in_window: true,
                },
            );
        }
        let _ = buffer.present();
    }

    /// Aeon's own shortcuts, from `[keys]`, plus Ctrl+1-9 to jump to a
    /// session. Returns whether the key was one of them.
    fn shortcut(&mut self, event: &KeyEvent, event_loop: &ActiveEventLoop) -> bool {
        #[derive(Clone, Copy)]
        enum Action {
            New,
            Close,
            Next,
            Previous,
            Sidebar,
            Settings,
            Find,
            Copy,
            Paste,
            Panel,
            SplitRight,
            SplitDown,
            Zoom,
            Rename,
            Finder,
            NewSessionMenu,
        }
        let (key, mods) = (&event.logical_key, self.modifiers);
        let keys = &self.config.keys;
        let action = [
            (&keys.new_session, Action::New),
            (&keys.close_session, Action::Close),
            (&keys.next_session, Action::Next),
            (&keys.previous_session, Action::Previous),
            (&keys.toggle_sidebar, Action::Sidebar),
            (&keys.settings, Action::Settings),
            (&keys.find, Action::Find),
            (&keys.copy, Action::Copy),
            (&keys.paste, Action::Paste),
            (&keys.project_panel, Action::Panel),
            (&keys.split_right, Action::SplitRight),
            (&keys.split_down, Action::SplitDown),
            (&keys.zoom_pane, Action::Zoom),
            (&keys.rename_session, Action::Rename),
            (&keys.palette, Action::Finder),
            (&keys.new_session_menu, Action::NewSessionMenu),
        ]
        .into_iter()
        .find(|(text, _)| text.parse::<Combo>().is_ok_and(|c| c.matches(key, mods)))
        .map(|(_, action)| action);
        match action {
            Some(Action::New) => self.new_session(),
            Some(Action::Close) => self.close_focused(event_loop),
            Some(Action::Next) => self.step(1),
            Some(Action::Previous) => self.step(-1),
            Some(Action::Sidebar) => self.toggle_sidebar(),
            Some(Action::Settings) => self.toggle_settings(),
            Some(Action::Find) => self.open_find(),
            Some(Action::Copy) => {
                self.copy();
            }
            Some(Action::Paste) => self.paste(),
            Some(Action::Panel) => self.toggle_panel(),
            Some(Action::SplitRight) => self.split(Split::Right),
            Some(Action::SplitDown) => self.split(Split::Down),
            Some(Action::Zoom) => self.toggle_zoom(),
            Some(Action::Rename) => self.start_rename(),
            Some(Action::Finder) => self.open_finder(false),
            Some(Action::NewSessionMenu) => self.open_finder(true),
            None => {
                if let Some(handled) = self.builtin_key(key, mods) {
                    return handled;
                }
                let digit = match key {
                    Key::Character(c) if mods == ModifiersState::CONTROL => c.parse::<usize>().ok(),
                    _ => None,
                };
                match digit {
                    Some(d @ 1..=9) => {
                        let order = chrome::display_order(&self.projects());
                        if let Some(&i) = order.get(d - 1) {
                            self.select(i);
                        }
                    }
                    _ => return false,
                }
            }
        }
        true
    }

    fn key(&mut self, event: &KeyEvent, event_loop: &ActiveEventLoop) {
        if event.state != ElementState::Pressed {
            return;
        }
        if self.finder.is_some() {
            self.finder_key(event);
            return;
        }
        if self.rename.is_some() {
            self.rename_key(event);
            return;
        }
        // While a shortcut is being recorded, every key belongs to the recorder.
        let recording = self.settings.as_ref().is_some_and(Settings::is_recording);
        if !recording && self.shortcut(event, event_loop) {
            return;
        }
        if let Some(panel) = &mut self.settings {
            let outcome = panel.key(&event.logical_key, self.modifiers, &self.config);
            self.settings_outcome(outcome);
            return;
        }
        if self.find.is_some() && self.find_key(event) {
            return;
        }
        let mods = self.modifiers;
        let Some(session) = self.sessions.get_mut(self.active) else {
            return;
        };
        let text = event.text_with_all_modifiers();
        let Some(bytes) = input::key_bytes(
            &event.logical_key,
            text,
            mods,
            session.term.modes().app_cursor,
        ) else {
            return;
        };
        // Typing returns to the prompt and drops the selection, as in other
        // terminals.
        session.scroll = 0;
        session.selection = None;
        // ConPTY drains its input pipe continuously; a keystroke-sized write
        // does not block the UI thread in practice.
        let _ = session.input.write_all(&bytes);
        self.request_redraw();
    }

    /// Edits the active session's name in the sidebar.
    fn start_rename(&mut self) {
        let Some(g) = self.active_group() else {
            return;
        };
        if !self.sidebar_visible {
            self.toggle_sidebar();
        }
        let title = self.group_title(g, &self.projects());
        self.rename = Some(Rename {
            group: g,
            text: title.clone(),
            original: title,
        });
        self.request_redraw();
    }

    /// Enter keeps the name, Escape drops it; an empty name goes back to
    /// the automatic title.
    fn rename_key(&mut self, event: &KeyEvent) {
        let Some(rename) = &mut self.rename else {
            return;
        };
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => self.rename = None,
            Key::Named(NamedKey::Enter) => self.commit_rename(),
            Key::Named(NamedKey::Backspace) => {
                rename.text.pop();
            }
            _ if self.modifiers.control_key() || self.modifiers.alt_key() => return,
            _ => {
                let Some(text) = event
                    .text
                    .as_ref()
                    .filter(|t| !t.chars().any(char::is_control))
                else {
                    return;
                };
                rename.text.push_str(text);
            }
        }
        self.request_redraw();
    }

    fn commit_rename(&mut self) {
        let Some(rename) = self.rename.take() else {
            return;
        };
        let name = rename.text.trim();
        if let Some(group) = self.groups.get_mut(rename.group)
            && name != rename.original
        {
            group.name = (!name.is_empty()).then(|| name.to_owned());
            self.save_state();
        }
        self.request_redraw();
    }

    /// Fixed keys: Ctrl+C copies a selection (and is ^C without one), Ctrl+V
    /// pastes, Ctrl+= Ctrl+- Ctrl+0 zoom, Shift+PageUp and Shift+PageDown
    /// scroll. `None` when the key is none of them.
    fn builtin_key(&mut self, key: &Key, mods: ModifiersState) -> Option<bool> {
        let ctrl = mods == ModifiersState::CONTROL;
        match key {
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("c") => {
                // Without a selection Ctrl+C belongs to the shell.
                Some(self.copy())
            }
            Key::Character(c) if ctrl && c.eq_ignore_ascii_case("v") => {
                self.paste();
                Some(true)
            }
            Key::Character(c) if ctrl && (c == "=" || c == "+") => {
                self.set_zoom(self.zoom + 1.0);
                Some(true)
            }
            Key::Character(c) if ctrl && c == "-" => {
                self.set_zoom(self.zoom - 1.0);
                Some(true)
            }
            Key::Character(c) if ctrl && c == "0" => {
                self.set_zoom(0.0);
                Some(true)
            }
            Key::Named(
                named @ (NamedKey::ArrowLeft
                | NamedKey::ArrowRight
                | NamedKey::ArrowUp
                | NamedKey::ArrowDown),
            ) if mods == ModifiersState::ALT => {
                let direction = match named {
                    NamedKey::ArrowLeft => Direction::Left,
                    NamedKey::ArrowRight => Direction::Right,
                    NamedKey::ArrowUp => Direction::Up,
                    _ => Direction::Down,
                };
                // With one pane the shell keeps Alt+Arrows.
                let shown: Vec<(SessionId, Rect)> = self
                    .shown_panes()
                    .into_iter()
                    .map(|(i, rect)| (self.sessions[i].id, rect))
                    .collect();
                if shown.len() < 2 {
                    return None;
                }
                let from = self.sessions.get(self.active)?.id;
                if let Some(index) =
                    panes::neighbor(&shown, from, direction).and_then(|id| self.session_index(id))
                {
                    self.focus_pane(index);
                }
                Some(true)
            }
            Key::Named(NamedKey::ArrowUp) if ctrl => {
                self.jump_prompt(-1);
                Some(true)
            }
            Key::Named(NamedKey::ArrowDown) if ctrl => {
                self.jump_prompt(1);
                Some(true)
            }
            Key::Named(NamedKey::PageUp) if mods == ModifiersState::SHIFT => {
                let rows = self
                    .sessions
                    .get(self.active)
                    .map_or(1, |s| s.term.grid().rows());
                self.scroll_by(rows as isize - 1);
                Some(true)
            }
            Key::Named(NamedKey::PageDown) if mods == ModifiersState::SHIFT => {
                let rows = self
                    .sessions
                    .get(self.active)
                    .map_or(1, |s| s.term.grid().rows());
                self.scroll_by(1 - rows as isize);
                Some(true)
            }
            _ => None,
        }
    }

    /// Scrolls the active session's view by `lines`, up when positive.
    fn scroll_by(&mut self, lines: isize) {
        let Some(session) = self.sessions.get_mut(self.active) else {
            return;
        };
        let max = session.term.scrollback_len() as isize;
        let next = (session.scroll as isize + lines).clamp(0, max) as usize;
        if next != session.scroll {
            session.scroll = next;
            self.link = None;
            self.request_redraw();
        }
    }

    fn set_zoom(&mut self, zoom: f32) {
        let zoom = zoom.clamp(6.0 - self.config.font.size, 48.0 - self.config.font.size);
        if zoom == self.zoom {
            return;
        }
        self.zoom = zoom;
        if let Some(view) = self.view.take() {
            let px = self.font_px(view.window.scale_factor());
            self.view = Some(View {
                atlas: view.atlas.with_px(px),
                ..view
            });
        }
        self.resize();
    }

    /// The terminal font's pixel size at `scale`, with this window's zoom.
    fn font_px(&self, scale: f64) -> f32 {
        points_to_px(self.config.font.size + self.zoom, scale)
    }

    /// Copies the selection to the clipboard and clears it. Returns whether
    /// there was one.
    fn copy(&mut self) -> bool {
        let Some(session) = self.sessions.get_mut(self.active) else {
            return false;
        };
        let Some((a, b)) = session.selection.and_then(|s| s.ordered()) else {
            return false;
        };
        let text = session.term.text(a, b);
        session.selection = None;
        let owner = self.view.as_ref().map_or(0, |v| hwnd(&v.window));
        sys::set_clipboard(owner, &text);
        self.request_redraw();
        true
    }

    fn paste(&mut self) {
        let Some(text) = sys::get_clipboard() else {
            return;
        };
        let Some(session) = self.sessions.get_mut(self.active) else {
            return;
        };
        let bytes = input::paste_bytes(&text, session.term.modes().bracketed_paste);
        session.scroll = 0;
        session.selection = None;
        let _ = session.input.write_all(&bytes);
        self.request_redraw();
    }

    fn open_find(&mut self) {
        if self.settings.is_some() {
            return;
        }
        if self.find.is_none() {
            self.find = Some(Find {
                query: String::new(),
                matches: Vec::new(),
                current: None,
            });
        }
        self.request_redraw();
    }

    /// Keys while the find bar is open. Returns whether it used the key.
    fn find_key(&mut self, event: &KeyEvent) -> bool {
        let shift = self.modifiers.shift_key();
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.find = None;
                self.request_redraw();
            }
            Key::Named(NamedKey::Enter) => self.find_step(if shift { 1 } else { -1 }),
            Key::Named(NamedKey::Backspace) => {
                if let Some(find) = &mut self.find {
                    find.query.pop();
                }
                self.find_refresh();
            }
            _ => {
                if self.modifiers.control_key() || self.modifiers.alt_key() {
                    return false;
                }
                let Some(text) = event
                    .text
                    .as_ref()
                    .filter(|t| !t.chars().any(char::is_control))
                else {
                    return false;
                };
                if let Some(find) = &mut self.find {
                    find.query.push_str(text);
                }
                self.find_refresh();
            }
        }
        true
    }

    /// Searches again and jumps to the newest match.
    fn find_refresh(&mut self) {
        let (Some(find), Some(session)) = (&mut self.find, self.sessions.get(self.active)) else {
            return;
        };
        find.matches = session.term.find(&find.query);
        find.current = find.matches.len().checked_sub(1);
        self.show_match();
    }

    /// Moves to the previous (older, `-1`) or next (`1`) match.
    fn find_step(&mut self, step: isize) {
        let Some(find) = &mut self.find else {
            return;
        };
        let n = find.matches.len() as isize;
        if n == 0 {
            return;
        }
        let at = find.current.map_or(n - 1, |c| c as isize);
        find.current = Some((at + step).rem_euclid(n) as usize);
        self.show_match();
    }

    /// Scrolls so the current match is on screen.
    fn show_match(&mut self) {
        let line = self
            .find
            .as_ref()
            .and_then(|f| f.current.and_then(|i| f.matches.get(i)))
            .map(|m| m.start.line);
        if let (Some(line), Some(session)) = (line, self.sessions.get_mut(self.active)) {
            let screen = session.term.screen_line();
            let rows = session.term.grid().rows() as u64;
            let top = screen.saturating_sub(session.scroll as u64);
            if line < top || line >= top + rows {
                session.scroll = if line >= screen {
                    0
                } else {
                    ((screen - line) + rows / 2).min(session.term.scrollback_len() as u64) as usize
                };
            }
        }
        self.request_redraw();
    }

    /// The focused pane's cell under (`x`, `y`): column and screen row,
    /// clamped to the grid. `None` outside that pane.
    fn cell_at(&self, x: f64, y: f64) -> Option<(usize, usize)> {
        if self.settings.is_some() {
            return None;
        }
        let view = self.view.as_ref()?;
        let (_, rect) = self
            .shown_panes()
            .into_iter()
            .find(|(i, rect)| *i == self.active && rect.contains(x, y))?;
        let ((left, top), _, _) = self.grid_in(view, rect);
        let grid = self.sessions.get(self.active)?.term.grid();
        let col = ((x - f64::from(left)).max(0.0) / f64::from(view.atlas.cell_width)) as usize;
        let row = ((y - f64::from(top)).max(0.0) / f64::from(view.atlas.cell_height)) as usize;
        Some((col.min(grid.cols() - 1), row.min(grid.rows() - 1)))
    }

    /// The pane on screen under (`x`, `y`).
    fn pane_at(&self, x: f64, y: f64) -> Option<usize> {
        if self.settings.is_some() {
            return None;
        }
        self.shown_panes()
            .into_iter()
            .find(|(_, rect)| rect.contains(x, y))
            .map(|(i, _)| i)
    }

    /// The history position of a screen cell, given the scroll.
    fn point(&self, (col, row): (usize, usize)) -> Option<Point> {
        let session = self.sessions.get(self.active)?;
        let top = session
            .term
            .screen_line()
            .saturating_sub(session.scroll as u64);
        Some(Point {
            line: top + row as u64,
            col,
        })
    }

    /// Whether mouse events go to the program rather than to selection:
    /// it asked for them, and Shift is not held to override it.
    fn reporting(&self) -> bool {
        !self.modifiers.shift_key()
            && self
                .sessions
                .get(self.active)
                .is_some_and(|s| s.term.modes().mouse != MouseMode::Off)
    }

    fn report(&mut self, button: Button, action: MouseAction, cell: (usize, usize)) {
        let mods = self.modifiers;
        let Some(session) = self.sessions.get_mut(self.active) else {
            return;
        };
        let bytes = input::mouse_report(
            button,
            action,
            mods,
            cell.0,
            cell.1,
            session.term.modes().sgr_mouse,
        );
        let _ = session.input.write_all(&bytes);
        self.reported = Some(cell);
    }

    /// A button pressed or released anywhere in the window.
    fn mouse(&mut self, state: ElementState, button: MouseButton) {
        let (x, y) = (self.cursor.x, self.cursor.y);
        let pressed = state == ElementState::Pressed;
        if self.finder.is_some() {
            if pressed && button == MouseButton::Left {
                self.finder_click();
            }
            return;
        }
        if pressed {
            self.commit_rename();
            // A click in another pane only moves the keyboard there.
            if let Some(index) = self.pane_at(x, y).filter(|&i| i != self.active) {
                self.focus_pane(index);
                return;
            }
        }
        let Some(cell) = self.cell_at(x, y) else {
            if pressed && button == MouseButton::Left {
                self.click();
            }
            return;
        };
        let which = match button {
            MouseButton::Left => Button::Left,
            MouseButton::Middle => Button::Middle,
            MouseButton::Right => Button::Right,
            _ => return,
        };
        if !pressed {
            if let Drag::Reporting(b) = self.drag {
                self.report(b, MouseAction::Release, cell);
            }
            self.drag = Drag::None;
            return;
        }
        if self.reporting() {
            self.report(which, MouseAction::Press, cell);
            self.drag = Drag::Reporting(which);
            return;
        }
        match which {
            Button::Left => self.press_left(cell),
            // Right-click copies a selection, or pastes, as in Windows Terminal.
            Button::Right if !self.copy() => self.paste(),
            _ => {}
        }
    }

    fn press_left(&mut self, cell: (usize, usize)) {
        if let Some(prompt) = self.hover_block
            && let Some(area) = self.active_term_area()
        {
            let (marks, _) = self.visible_blocks(self.active);
            let (x, y) = (self.cursor.x, self.cursor.y);
            if let Some(mark) = marks.iter().find(|m| m.prompt == prompt)
                && let Some(view) = self.view.as_mut()
                && let Some(action) = chrome::block_action_at(&mut view.ui, &area, mark, x, y)
            {
                self.block_action(prompt, action);
                return;
            }
        }
        if self.modifiers.control_key()
            && let Some((_, _, _, target)) = self.link.clone()
        {
            self.open_link(&target);
            return;
        }
        let Some(point) = self.point(cell) else {
            return;
        };
        let now = Instant::now();
        let count = match self.last_click {
            Some((at, p, n)) if now - at < MULTI_CLICK && p == point => n % 3 + 1,
            _ => 1,
        };
        self.last_click = Some((now, point, count));
        let Some(session) = self.sessions.get_mut(self.active) else {
            return;
        };
        session.selection = Some(match count {
            2 => {
                let (anchor, head) = session.term.word_at(point);
                Selection {
                    anchor,
                    head,
                    shown: true,
                }
            }
            3 => {
                let width = session.term.grid().cols().saturating_sub(1);
                Selection {
                    anchor: Point {
                        line: point.line,
                        col: 0,
                    },
                    head: Point {
                        line: point.line,
                        col: width,
                    },
                    shown: true,
                }
            }
            _ => Selection {
                anchor: point,
                head: point,
                shown: false,
            },
        });
        self.drag = Drag::Selecting;
        self.request_redraw();
    }

    /// The mouse moved with a button held, or with any-motion tracking on.
    fn drag_to(&mut self) {
        let Some(cell) = self.cell_at(self.cursor.x, self.cursor.y) else {
            return;
        };
        match self.drag {
            Drag::Selecting => {
                let Some(point) = self.point(cell) else {
                    return;
                };
                if let Some(selection) = self
                    .sessions
                    .get_mut(self.active)
                    .and_then(|s| s.selection.as_mut())
                    && selection.head != point
                {
                    selection.head = point;
                    selection.shown = true;
                    self.request_redraw();
                }
            }
            Drag::Reporting(button) => {
                let mode = self.sessions.get(self.active).map(|s| s.term.modes().mouse);
                if matches!(mode, Some(MouseMode::Drag | MouseMode::Motion))
                    && self.reported != Some(cell)
                {
                    self.report(button, MouseAction::Motion, cell);
                }
            }
            Drag::None => {
                let mode = self.sessions.get(self.active).map(|s| s.term.modes().mouse);
                if mode == Some(MouseMode::Motion)
                    && self.reporting()
                    && self.reported != Some(cell)
                {
                    self.report(Button::None, MouseAction::Motion, cell);
                }
            }
        }
    }

    fn wheel(&mut self, delta: MouseScrollDelta) {
        let cell_height = self
            .view
            .as_ref()
            .map_or(16.0, |v| f64::from(v.atlas.cell_height));
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => {
                (f64::from(y) * WHEEL_LINES as f64).round() as isize
            }
            MouseScrollDelta::PixelDelta(p) => (p.y / cell_height).round() as isize,
        };
        if lines == 0 {
            return;
        }
        if self.modifiers.control_key() {
            self.set_zoom(self.zoom + lines.signum() as f32);
            return;
        }
        let Some(cell) = self.cell_at(self.cursor.x, self.cursor.y) else {
            return;
        };
        if self.reporting() {
            let button = if lines > 0 {
                Button::WheelUp
            } else {
                Button::WheelDown
            };
            for _ in 0..lines.unsigned_abs().min(WHEEL_LINES) {
                self.report(button, MouseAction::Press, cell);
            }
            return;
        }
        let Some(session) = self.sessions.get_mut(self.active) else {
            return;
        };
        let modes = session.term.modes();
        if modes.alt_screen {
            // Full-screen programs without mouse tracking scroll with arrows.
            let key = if lines > 0 {
                NamedKey::ArrowUp
            } else {
                NamedKey::ArrowDown
            };
            if let Some(bytes) = input::key_bytes(
                &Key::Named(key),
                None,
                ModifiersState::empty(),
                modes.app_cursor,
            ) {
                for _ in 0..lines.unsigned_abs() {
                    let _ = session.input.write_all(&bytes);
                }
            }
            return;
        }
        self.scroll_by(lines);
    }

    /// Opens a URL in the browser, or a file in VS Code at its line when
    /// `code` is on PATH and in its default app otherwise.
    fn open_link(&self, target: &Target) {
        match target {
            Target::Url(url) => {
                sys::open_url(url);
            }
            Target::File { path, line, col } => {
                let cwd = self
                    .sessions
                    .get(self.active)
                    .and_then(|s| s.context.cwd.clone());
                let full = match cwd {
                    Some(cwd) if std::path::Path::new(path).is_relative() => cwd.join(path),
                    _ => PathBuf::from(path),
                };
                if !full.exists() {
                    return;
                }
                let spot = match (line, col) {
                    (Some(l), Some(c)) => format!("{}:{l}:{c}", full.display()),
                    (Some(l), None) => format!("{}:{l}", full.display()),
                    _ => full.display().to_string(),
                };
                if line.is_some()
                    && let Some(code) = find_on_path("code.cmd")
                {
                    let _ = hidden(std::process::Command::new(code).args(["-g", &spot])).spawn();
                } else {
                    sys::open_url(&full.to_string_lossy());
                }
            }
        }
    }

    /// With Ctrl held, the link under the mouse, if any.
    fn hover_link(&mut self) {
        let link = self
            .modifiers
            .control_key()
            .then(|| self.cell_at(self.cursor.x, self.cursor.y))
            .flatten()
            .filter(|_| !self.reporting())
            .and_then(|cell| {
                let point = self.point(cell)?;
                let session = self.sessions.get(self.active)?;
                let chars: Vec<char> = session
                    .term
                    .line(point.line)?
                    .iter()
                    .map(|c| c.ch)
                    .collect();
                let link = links::link_at(&chars, point.col)?;
                Some((point.line, link.start, link.end, link.target))
            });
        let changed = link.as_ref().map(|(l, a, b, _)| (*l, *a, *b))
            != self.link.as_ref().map(|(l, a, b, _)| (*l, *a, *b));
        self.link = link;
        if changed {
            self.request_redraw();
        }
    }

    fn click(&mut self) {
        let Some(view) = &self.view else {
            return;
        };
        let scale = view.window.scale_factor();
        let size = view.window.inner_size();
        let (x, y) = (self.cursor.x, self.cursor.y);
        let layout = self.layout(view);
        if x >= f64::from(layout.sidebar) {
            if let Some(panel) = &mut self.settings {
                let geo = settings::Geometry::new(layout.sidebar, size.width, scale);
                let outcome = panel.click(x, y, &geo, &self.config);
                self.settings_outcome(outcome);
                return;
            }
            match self.right_hit(x, y) {
                (Some(HeaderHit::TogglePanel), _) => self.toggle_panel(),
                (Some(HeaderHit::OpenEditor), _) => self.open_editor(),
                (_, Some(hit)) => self.panel_action(hit),
                _ => {}
            }
            return;
        }
        match self.sidebar_hit(x, y) {
            Some(Hit::Settings) => self.toggle_settings(),
            Some(Hit::Update) => {
                let command = match self.update {
                    Status::Available(_) => Some(ResidentCommand::DownloadUpdate),
                    Status::Ready { .. } => Some(ResidentCommand::InstallUpdate),
                    Status::Error(_) => Some(ResidentCommand::CheckUpdates),
                    _ => None,
                };
                if let Some(command) = command {
                    self.update_command(command);
                }
            }
            Some(Hit::NewSession) => self.new_session(),
            Some(Hit::NewSessionMenu) => self.open_finder(true),
            Some(Hit::Project(i)) => {
                let projects = self.projects();
                let Some(project) = projects.get(i) else {
                    return;
                };
                self.settings = None;
                match project.sessions.first() {
                    Some(&first) => self.select(first),
                    None => {
                        if let Err(e) = self.open_session(project.path.clone(), None) {
                            eprintln!("aeon: could not open a session: {e}");
                        }
                    }
                }
                self.request_redraw();
            }
            Some(Hit::Pin(i)) => {
                if let Some(path) = self.projects().get(i).and_then(|p| p.path.clone()) {
                    if let Some(at) = self.pinned.iter().position(|p| *p == path) {
                        self.pinned.remove(at);
                    } else {
                        self.pinned.push(path);
                    }
                    self.save_state();
                    self.request_redraw();
                }
            }
            Some(Hit::Mute(g)) => {
                if let Some(group) = self.groups.get(g) {
                    let muted = !self.lead(group).is_some_and(|s| s.muted);
                    for id in group.tree.panes() {
                        if let Some(i) = self.session_index(id) {
                            self.sessions[i].muted = muted;
                        }
                    }
                }
                self.request_redraw();
            }
            Some(Hit::Session(i)) => {
                self.settings = None;
                self.select(i);
                self.request_redraw();
            }
            None => {}
        }
    }

    /// Shows a hand over anything a click acts on, and highlights what the
    /// mouse is over in the sidebar.
    fn update_pointer(&mut self) {
        let (x, y) = (self.cursor.x, self.cursor.y);
        let hover = self.sidebar_hit(x, y);
        if hover != self.hover {
            self.hover = hover;
            self.request_redraw();
        }
        let (header_hover, panel_hover) = self.right_hit(x, y);
        if header_hover != self.header_hover || panel_hover != self.panel_hover {
            self.header_hover = header_hover;
            self.panel_hover = panel_hover;
            self.request_redraw();
        }
        let hover = hover.is_some() || header_hover.is_some() || panel_hover.is_some();
        let Some(view) = &self.view else {
            return;
        };
        let layout = self.layout(view);
        let scale = view.window.scale_factor();
        let size = view.window.inner_size();
        let clickable = hover
            || (x >= f64::from(layout.sidebar)
                && self.settings.as_ref().is_some_and(|panel| {
                    let geo = settings::Geometry::new(layout.sidebar, size.width, scale);
                    panel.is_clickable(x, y, &geo)
                }));
        let in_terminal = self.cell_at(x, y).is_some();
        self.hover_link();
        let block = self
            .cell_at(x, y)
            .and_then(|cell| self.point(cell))
            .and_then(|point| {
                let (marks, _) = self.visible_blocks(self.active);
                marks
                    .iter()
                    .find(|m| m.prompt == point.line)
                    .map(|m| m.prompt)
            });
        if block != self.hover_block {
            self.hover_block = block;
            self.request_redraw();
        }
        let icon = if clickable || self.link.is_some() {
            CursorIcon::Pointer
        } else if in_terminal && !self.reporting() {
            CursorIcon::Text
        } else {
            CursorIcon::Default
        };
        if let Some(view) = &self.view {
            view.window.set_cursor(icon);
        }
        self.pointer = clickable;
    }

    fn toggle_settings(&mut self) {
        self.settings = match self.settings {
            Some(_) => None,
            None => Some(Settings::new(&self.config, self.update.clone())),
        };
        self.request_redraw();
    }

    fn settings_outcome(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::None => {}
            Outcome::Redraw => self.request_redraw(),
            Outcome::Close => {
                self.settings = None;
                self.request_redraw();
            }
            Outcome::OpenConfigFile => crate::launcher::open_settings(),
            Outcome::Update(command) => self.update_command(command),
            Outcome::Save(changes) => {
                let result = self.save_settings(&changes);
                if let Some(panel) = &mut self.settings {
                    panel.error = result.err();
                    panel.rebuild(&self.config);
                }
                self.request_redraw();
            }
        }
    }

    /// Writes `changes` to config.toml and applies the new config: theme and
    /// font now, the shell for sessions opened from now on, and the launcher
    /// through the resident.
    fn save_settings(&mut self, changes: &[aeon_config::edit::Change]) -> Result<(), String> {
        let path = Config::default_path().ok_or("no config folder (APPDATA is not set)")?;
        aeon_config::edit::write(&path, changes)?;
        let config = Config::load(&path).map_err(|e| e.to_string())?;
        let font_changed = config.font != self.config.font;
        if font_changed && let Some(view) = &mut self.view {
            let font = font::load(config.font.path.as_deref())?;
            let px = points_to_px(config.font.size + self.zoom, view.window.scale_factor());
            view.atlas = Atlas::new(Rc::new(font), px);
        }
        self.theme = Theme::new(&config.colors);
        self.palette = Palette::new(&self.theme);
        if let Some(view) = &self.view {
            view.window.set_theme(Some(window_theme(&self.theme)));
        }
        self.config = config;
        self.resize();
        aeon_launcher::sys::notify_settings_changed();
        Ok(())
    }

    /// The update notice in the sidebar's footer, when there is one.
    fn pill(&self) -> Option<Pill> {
        match &self.update {
            Status::Available(_) => Some(Pill::Available),
            Status::Downloading { percent, .. } => Some(Pill::Downloading(*percent)),
            Status::Ready { .. } => Some(Pill::Ready),
            Status::Error(_) => Some(Pill::Failed),
            Status::Checking => Some(Pill::Checking),
            Status::Idle | Status::UpToDate => None,
        }
    }

    /// Asks the resident to act on updates, starting one when none runs:
    /// it checks as soon as it is up.
    fn update_command(&mut self, command: ResidentCommand) {
        if !sys::send_to_resident(command) {
            let _ = std::env::current_exe().and_then(|exe| {
                std::process::Command::new(exe)
                    .args(["--background", "--check-updates"])
                    .spawn()
                    .map(drop)
            });
        }
        self.update_watch_until = Some(Instant::now() + Duration::from_secs(5));
    }

    /// Re-reads the update status, redrawing only when it changed.
    fn refresh_update(&mut self) {
        let status = updater::read_status();
        if status == self.update {
            return;
        }
        self.update = status;
        if let Some(panel) = &mut self.settings {
            panel.update = self.update.clone();
            panel.rebuild(&self.config);
        }
        self.request_redraw();
    }

    /// Opens the first sessions: the last run's when this is the only
    /// window and nothing was asked to run, plus one where Aeon was started
    /// from if none of them is there.
    fn start(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let launch = self.launch.take().unwrap_or_default();
        let (saved, active) = self.restore.take().unwrap_or_default();
        let plain = launch.run.is_none() && launch.program.is_none() && launch.cwd.is_none();
        if plain && self.primary && self.config.window.restore {
            for group in &saved {
                self.restore_group(group);
            }
        }
        if self.groups.is_empty() {
            return self.open_with(launch.cwd, launch.run, launch.program);
        }
        let here = start_dir().filter(|dir| Some(dir) != self.home.as_ref());
        let found = here.as_ref().and_then(|dir| {
            self.groups.iter().position(|g| {
                g.tree.panes().into_iter().any(|id| {
                    self.session_index(id)
                        .is_some_and(|i| self.sessions[i].context.cwd.as_ref() == Some(dir))
                })
            })
        });
        match (here, found) {
            (_, Some(g)) => self.select(g),
            (Some(dir), None) => self.open_session(Some(dir), None)?,
            (None, None) => self.select(active.min(self.groups.len() - 1)),
        }
        self.resize();
        Ok(())
    }

    /// Opens a saved session's panes, each in the folder it was in.
    fn restore_group(&mut self, saved: &state::Group) {
        let Some(view) = self.view.as_ref() else {
            return;
        };
        let Some(mut tree) = Tree::from_shape(&saved.shape, &mut (self.next_id..)) else {
            return;
        };
        let ids = tree.panes();
        if ids.len() != saved.panes.len() {
            return;
        }
        let (mut rects, mut dividers) = (Vec::new(), Vec::new());
        tree.layout(
            self.layout(view).area,
            divider(view),
            &mut rects,
            &mut dividers,
        );
        let sizes: Vec<(usize, usize)> = rects
            .iter()
            .map(|(_, rect)| {
                let (_, rows, cols) = self.grid_in(view, *rect);
                (rows, cols)
            })
            .collect();
        self.next_id += ids.len() as u64;
        for ((&id, cwd), size) in ids.iter().zip(&saved.panes).zip(sizes) {
            let cwd = cwd.is_dir().then(|| cwd.clone());
            if let Err(e) = self.spawn_pane(id, cwd, None, None, size) {
                eprintln!("aeon: could not restore a session: {e}");
                if !tree.remove(id) {
                    return;
                }
            }
        }
        let focus = tree.panes()[0];
        self.groups.push(Group {
            tree,
            focus,
            zoomed: false,
            name: saved.name.clone(),
        });
    }

    /// The sessions as the state file keeps them.
    fn saved_groups(&self) -> Vec<state::Group> {
        self.groups
            .iter()
            .map(|g| state::Group {
                name: g.name.clone(),
                shape: g.tree.shape(),
                panes: g
                    .tree
                    .panes()
                    .into_iter()
                    .map(|id| {
                        self.session_index(id)
                            .and_then(|i| self.sessions[i].context.cwd.clone())
                            .unwrap_or_default()
                    })
                    .collect(),
            })
            .collect()
    }

    /// Opens the palette over the window: its sessions, the active project's
    /// tasks, new sessions, apps, commands, math, and the web. `menu` opens
    /// only the new-session profiles.
    fn open_finder(&mut self, menu: bool) {
        self.rename = None;
        let projects = self.projects();
        let sessions = chrome::display_order(&projects)
            .into_iter()
            .map(|g| Entry {
                id: g,
                title: self.group_title(g, &projects),
                detail: projects
                    .iter()
                    .find(|p| p.sessions.contains(&g))
                    .map(|p| p.name.clone())
                    .unwrap_or_default(),
            })
            .collect();
        let tasks = self
            .sessions
            .get(self.active)
            .map(|s| {
                s.context
                    .tasks
                    .iter()
                    .enumerate()
                    .map(|(i, t)| Entry {
                        id: i,
                        title: t.name.clone(),
                        detail: t.command.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        // A few hundred kilobytes at most, read once per open, as the
        // launcher does.
        let history = if menu {
            Vec::new()
        } else {
            history::dir()
                .map(|d| history::load(&d))
                .unwrap_or_default()
        };
        let catalog = self.catalog.as_ref();
        let sources = Sources {
            history,
            apps: catalog.map(|c| c.apps.clone()).unwrap_or_default(),
            commands: catalog.map(|c| c.commands.clone()).unwrap_or_default(),
            search_url: self.config.launcher.search_url.clone(),
            sessions,
            tasks,
            profiles: catalog.map(|c| c.profiles.clone()).unwrap_or_default(),
        };
        self.finder = Some(if menu {
            Launcher::menu(sources)
        } else {
            Launcher::open(sources)
        });
        self.finder_error = None;
        if self
            .catalog_requested
            .is_none_or(|t| t.elapsed() >= CATALOG_MAX_AGE)
        {
            self.catalog_requested = Some(Instant::now());
            let proxy = self.proxy.clone();
            let _ = std::thread::Builder::new()
                .name("catalog".into())
                .spawn(move || {
                    let _ = proxy.send_event(Event::Catalog(Catalog {
                        apps: apps::load(),
                        commands: apps::path_commands(),
                        profiles: aeon_launcher::profiles::detect(),
                    }));
                });
        }
        self.request_redraw();
    }

    /// Where the palette sits: centred, near the top.
    fn finder_rect(&self) -> Option<Rect> {
        let finder = self.finder.as_ref()?;
        let view = self.view.as_ref()?;
        let size = view.window.inner_size();
        let w = view
            .ui
            .px(FINDER_WIDTH)
            .min(size.width.saturating_sub(view.ui.px(32.0)));
        Some(Rect {
            x: (size.width - w) / 2,
            y: (size.height / 8).max(view.ui.px(56.0)),
            w,
            h: finder::height(&view.ui, finder.items().len()),
        })
    }

    fn finder_key(&mut self, event: &KeyEvent) {
        let ctrl = self.modifiers.control_key();
        let alt = self.modifiers.alt_key();
        let Some(launcher) = &mut self.finder else {
            return;
        };
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => self.finder = None,
            Key::Named(NamedKey::Enter) => {
                if let Some(action) = launcher.choose(ctrl) {
                    self.perform(action);
                }
                return;
            }
            Key::Named(NamedKey::ArrowDown) => launcher.select_next(),
            Key::Named(NamedKey::ArrowUp) => launcher.select_previous(),
            Key::Named(NamedKey::Tab) => launcher.complete(),
            Key::Named(NamedKey::Backspace) if ctrl => launcher.delete_word(),
            Key::Named(NamedKey::Backspace) => launcher.backspace(),
            Key::Character(c) if ctrl && !alt && c.eq_ignore_ascii_case("v") => {
                if let Some(text) = sys::get_clipboard() {
                    launcher.insert(&text);
                }
            }
            // AltGr reports Ctrl+Alt, so typed text is taken whatever the
            // modifiers; control characters (Ctrl+letter) are dropped.
            _ => match &event.text {
                Some(text) if !text.chars().any(char::is_control) => launcher.insert(text),
                _ => return,
            },
        }
        self.finder_error = None;
        self.request_redraw();
    }

    /// A click on a result chooses it; one outside the palette closes it.
    fn finder_click(&mut self) {
        let (Some(rect), Some(view)) = (self.finder_rect(), self.view.as_ref()) else {
            return;
        };
        let (x, y) = (self.cursor.x, self.cursor.y);
        let rows = self.finder.as_ref().map_or(0, |f| f.items().len());
        if let Some(row) = finder::row_at(&view.ui, rect, rows, x, y) {
            let ctrl = self.modifiers.control_key();
            let action = self.finder.as_mut().and_then(|f| {
                f.select(row);
                f.choose(ctrl)
            });
            if let Some(action) = action {
                self.perform(action);
            }
        } else if !rect.contains(x, y) {
            self.finder = None;
            self.request_redraw();
        }
    }

    /// Carries out what the palette chose, then closes it; on failure it
    /// stays open and says why.
    fn perform(&mut self, action: Action) {
        let cwd = self
            .sessions
            .get(self.active)
            .and_then(|s| s.context.cwd.clone())
            .or_else(start_dir)
            .unwrap_or_else(|| PathBuf::from("C:\\"));
        let result: Result<(), String> = match &action {
            Action::Copy(text) => {
                let owner = self.view.as_ref().map_or(0, |v| hwnd(&v.window));
                if sys::set_clipboard(owner, text) {
                    Ok(())
                } else {
                    Err("the clipboard is busy".into())
                }
            }
            // In a window, Enter runs a command at the focused prompt.
            Action::Run(command) => {
                if let Some(session) = self.sessions.get_mut(self.active) {
                    session.scroll = 0;
                    session.selection = None;
                    let mut bytes = command.clone().into_bytes();
                    bytes.push(b'\r');
                    let _ = session.input.write_all(&bytes);
                }
                Ok(())
            }
            Action::RunInTerminal(command) => self
                .open_session(Some(cwd.clone()), Some(command.clone()))
                .map_err(|e| e.to_string()),
            Action::OpenApp(id) => apps::open(id).map_err(|e| e.to_string()),
            Action::OpenUrl(url) => {
                if sys::open_url(url) {
                    Ok(())
                } else {
                    Err("no browser opened it".into())
                }
            }
            Action::SetTheme(theme) => {
                let light = match theme {
                    aeon_launcher::Theme::Toggle => !sys::light_theme().unwrap_or(false),
                    aeon_launcher::Theme::Dark => false,
                    aeon_launcher::Theme::Light => true,
                };
                // The change broadcast waits on every window.
                std::thread::spawn(move || sys::set_light_theme(light));
                Ok(())
            }
            Action::GoToSession(g) => {
                self.settings = None;
                self.select(*g);
                Ok(())
            }
            Action::RunTask(i) => {
                self.run_task(*i);
                Ok(())
            }
            Action::NewSession(profile) if profile.kind == Kind::Admin => {
                crate::launcher::open_profile(profile, &cwd).map_err(|e| e.to_string())
            }
            Action::NewSession(profile) => self
                .open_with(
                    Some(cwd.clone()),
                    None,
                    Some(Program {
                        program: profile.program.clone(),
                        args: profile.args.clone(),
                    }),
                )
                .map_err(|e| e.to_string()),
        };
        match result {
            Ok(()) => {
                match (&action, history::dir()) {
                    (Action::RunInTerminal(command), Some(dir)) => {
                        let _ = history::append(&dir, command);
                    }
                    (Action::OpenApp(id), Some(dir)) => {
                        let _ = history::record_app(&dir, id);
                    }
                    _ => {}
                }
                self.finder = None;
            }
            Err(e) => self.finder_error = Some(format!("Could not start: {e}")),
        }
        self.request_redraw();
    }

    fn session_index(&self, id: SessionId) -> Option<usize> {
        self.sessions.iter().position(|s| s.id == id)
    }
}

impl ApplicationHandler<Event> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.view.is_some() {
            return;
        }
        let started = self.create_view(event_loop).and_then(|view| {
            self.view = Some(view);
            self.start()
        });
        if let Err(e) = started {
            eprintln!("aeon: failed to start: {e}");
            event_loop.exit();
        }
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if let StartCause::ResumeTimeReached { .. } = cause {
            // about_to_wait schedules the next read, if one is still needed.
            event_loop.set_control_flow(ControlFlow::Wait);
            self.refresh_update();
        }
    }

    /// Polls the update status only while a check or download is under way.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let watching = self.update_watch_until.is_some_and(|t| now < t);
        let busy = matches!(self.update, Status::Checking | Status::Downloading { .. });
        if !watching {
            self.update_watch_until = None;
        }
        match event_loop.control_flow() {
            ControlFlow::WaitUntil(_) if watching || busy => {}
            _ if watching || busy => {
                event_loop.set_control_flow(ControlFlow::WaitUntil(now + UPDATE_POLL))
            }
            _ => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        match event {
            Event::Editor(path) => {
                self.editor = path;
                self.request_redraw();
            }
            Event::Output(id, bytes) => {
                let Some(index) = self.session_index(id) else {
                    return;
                };
                let shown = self.is_shown(index);
                let session = &mut self.sessions[index];
                session.term.advance(&bytes);
                let scrolled = session.term.take_scrolled();
                if session.term.modes().alt_screen {
                    session.scroll = 0;
                } else if session.scroll > 0 {
                    // Stay on the same text while new lines arrive below.
                    session.scroll = (session.scroll + scrolled).min(session.term.scrollback_len());
                }
                self.shell_events(index);
                let session = &mut self.sessions[index];
                let mut redraw = session.term.take_dirty() && shown;
                // The first report usually repeats the starting directory,
                // which the bar already shows in full.
                if session.term.take_cwd_changed()
                    && let Some(cwd) = session.term.cwd().map(PathBuf::from)
                    && session.context.cwd.as_ref() != Some(&cwd)
                {
                    // Show the new directory now; git and servers follow
                    // from the watcher.
                    session.context.cwd = Some(cwd.clone());
                    session.context.git = None;
                    if let Some(watcher) = &session.watcher {
                        watcher.set_cwd(cwd);
                    }
                    if index == self.active {
                        self.update_title();
                    }
                    // The sidebar shows every session's folder.
                    redraw = true;
                }
                if redraw {
                    self.request_redraw();
                }
            }
            Event::Context(id, context) => {
                let Some(index) = self.session_index(id) else {
                    return;
                };
                let session = &mut self.sessions[index];
                // The watcher may answer for a directory the shell already
                // left; never let it overwrite a newer one.
                if context.cwd == session.context.cwd {
                    session.context = context;
                    self.request_redraw();
                }
            }
            Event::Exited(id) => {
                if let Some(index) = self.session_index(id) {
                    self.close_pane(index, event_loop);
                }
            }
            Event::Catalog(mut catalog) => {
                if let Some(dir) = history::dir() {
                    apps::count_uses(&mut catalog.apps, &history::app_uses(&dir));
                }
                if let Some(finder) = &mut self.finder {
                    finder.set_profiles(catalog.profiles.clone());
                    finder.set_catalog(catalog.apps.clone(), catalog.commands.clone());
                    self.request_redraw();
                }
                self.catalog = Some(catalog);
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                let all: Vec<usize> = (0..self.sessions.len()).collect();
                if self.confirm_close(&all) {
                    self.quit(event_loop);
                }
            }
            WindowEvent::Resized(_) => self.resize(),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(mut view) = self.view.take() {
                    let px = self.font_px(scale_factor);
                    view.ui.set_scale(scale_factor);
                    self.view = Some(View {
                        atlas: view.atlas.with_px(px),
                        ..view
                    });
                    self.resize();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Focused(focused) => {
                self.focused = focused;
                // The cursor turns hollow while the window is in the background.
                self.request_redraw();
                if focused {
                    self.refresh_update();
                    self.mark_seen();
                }
            }
            WindowEvent::ModifiersChanged(m) => {
                self.modifiers = m.state();
                self.update_pointer();
            }
            WindowEvent::KeyboardInput { event, .. } => self.key(&event, event_loop),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = position;
                self.update_pointer();
                self.drag_to();
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = PhysicalPosition::new(-1.0, -1.0);
                if self.hover.take().is_some() {
                    self.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => self.mouse(state, button),
            WindowEvent::MouseWheel { delta, .. } => self.wheel(delta),
            _ => {}
        }
    }
}

/// Small UI state that outlives a window, kept apart from config.toml so
/// Aeon never rewrites the user's own file (and its comments).
mod state {
    use std::path::PathBuf;

    /// What the window remembers between runs.
    pub struct Saved {
        pub sidebar: bool,
        pub panel: bool,
        pub pinned: Vec<PathBuf>,
        /// The first window's sessions when it closed, to open again.
        pub sessions: Vec<Group>,
        /// Which of them was shown.
        pub active: usize,
    }

    /// A saved session: its name, how its panes were split, and the folder
    /// each pane was in.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Group {
        pub name: Option<String>,
        /// [`crate::panes::Tree::shape`].
        pub shape: String,
        pub panes: Vec<PathBuf>,
    }

    fn path() -> Option<PathBuf> {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("aeon").join("state"))
    }

    pub fn load() -> Saved {
        let text = path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        parse(&text)
    }

    pub fn parse(text: &str) -> Saved {
        let mut saved = Saved {
            sidebar: true,
            panel: false,
            pinned: Vec::new(),
            sessions: Vec::new(),
            active: 0,
        };
        for line in text.lines() {
            match line.trim().split_once('=') {
                Some(("sidebar", v)) => saved.sidebar = v != "hidden",
                Some(("panel", v)) => saved.panel = v == "shown",
                Some(("pinned", v)) if !v.is_empty() => saved.pinned.push(PathBuf::from(v)),
                Some(("session", v)) => saved.sessions.push(Group {
                    name: (!v.is_empty()).then(|| v.to_owned()),
                    shape: ".".to_owned(),
                    panes: Vec::new(),
                }),
                Some(("shape", v)) => {
                    if let Some(group) = saved.sessions.last_mut() {
                        group.shape = v.to_owned();
                    }
                }
                Some(("pane", v)) => {
                    if let Some(group) = saved.sessions.last_mut() {
                        group.panes.push(PathBuf::from(v));
                    }
                }
                Some(("active", v)) => saved.active = v.parse().unwrap_or(0),
                _ => {}
            }
        }
        saved
    }

    pub fn format(saved: &Saved) -> String {
        let shown = |on: bool| if on { "shown" } else { "hidden" };
        let mut text = format!(
            "sidebar={}\npanel={}\n",
            shown(saved.sidebar),
            shown(saved.panel)
        );
        for path in &saved.pinned {
            text.push_str(&format!("pinned={}\n", path.display()));
        }
        for group in &saved.sessions {
            text.push_str(&format!(
                "session={}\nshape={}\n",
                group.name.as_deref().unwrap_or_default(),
                group.shape
            ));
            for pane in &group.panes {
                text.push_str(&format!("pane={}\n", pane.display()));
            }
        }
        if !saved.sessions.is_empty() {
            text.push_str(&format!("active={}\n", saved.active));
        }
        text
    }

    /// Writes `saved` with `sessions`, or with the sessions already in the
    /// file when there are none to give.
    pub fn save(mut saved: Saved, sessions: Option<(Vec<Group>, usize)>) {
        let Some(path) = path() else { return };
        (saved.sessions, saved.active) = match sessions {
            Some(sessions) => sessions,
            None => {
                let old = load();
                (old.sessions, old.active)
            }
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, format(&saved));
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn state_round_trips_and_reads_old_files() {
            let saved = Saved {
                sidebar: false,
                panel: true,
                pinned: vec![PathBuf::from(r"C:\code\api")],
                sessions: vec![
                    Group {
                        name: Some("api server".into()),
                        shape: "r..".into(),
                        panes: vec![PathBuf::from(r"C:\code\api"), PathBuf::from(r"C:\code")],
                    },
                    Group {
                        name: None,
                        shape: ".".into(),
                        panes: vec![PathBuf::from(r"C:\Users\me")],
                    },
                ],
                active: 1,
            };
            let back = parse(&format(&saved));
            assert!(!back.sidebar && back.panel);
            assert_eq!(back.pinned, saved.pinned);
            assert_eq!(back.sessions, saved.sessions);
            assert_eq!(back.active, 1);
            // The file before this version held only the sidebar.
            let old = parse("sidebar=hidden\n");
            assert!(!old.sidebar && !old.panel && old.pinned.is_empty());
            assert!(old.sessions.is_empty());
        }
    }
}

/// Launched from a shell, Aeon starts where that shell was. Launched from the
/// Start menu or Explorer, the working directory is the install or system
/// folder, which no one wants a prompt in, so start at home instead.
pub(crate) fn start_dir() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_owned();
    let windir = std::env::var_os("WINDIR").map(PathBuf::from);
    let incidental = cwd == exe_dir || windir.is_some_and(|w| cwd.starts_with(w));
    if incidental {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    } else {
        Some(cwd)
    }
}

fn hwnd(window: &Window) -> isize {
    match window.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(handle)) => handle.hwnd.get(),
        _ => 0,
    }
}

fn find_on_path(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(exe))
        .find(|candidate| candidate.is_file())
}

/// A child process without a console window flashing up.
fn hidden(command: &mut std::process::Command) -> &mut std::process::Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW)
}

/// Dark title bar for a dark theme, light for a light one.
fn window_theme(theme: &Theme) -> winit::window::Theme {
    let luma = |c: u32| {
        0.299 * f64::from((c >> 16) & 0xff)
            + 0.587 * f64::from((c >> 8) & 0xff)
            + 0.114 * f64::from(c & 0xff)
    };
    if luma(theme.bg) < 128.0 {
        winit::window::Theme::Dark
    } else {
        winit::window::Theme::Light
    }
}

pub(crate) fn points_to_px(points: f32, scale_factor: f64) -> f32 {
    (f64::from(points) * 96.0 / 72.0 * scale_factor) as f32
}

/// The width of the line between panes.
fn divider(view: &View) -> u32 {
    view.ui.px(1.0).max(1)
}

fn pty_size(rows: usize, cols: usize) -> PtySize {
    PtySize {
        rows: rows.min(usize::from(u16::MAX)) as u16,
        cols: cols.min(usize::from(u16::MAX)) as u16,
    }
}

/// Reads PTY output on its own thread so a chatty shell never stalls input or
/// painting. Each chunk wakes the event loop once.
fn spawn_reader(id: SessionId, mut output: File, proxy: EventLoopProxy<Event>) {
    std::thread::Builder::new()
        .name(format!("pty-reader-{id}"))
        .spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match output.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if proxy
                            .send_event(Event::Output(id, buf[..n].to_vec()))
                            .is_err()
                        {
                            return;
                        }
                    }
                }
            }
            let _ = proxy.send_event(Event::Exited(id));
        })
        .expect("spawning the pty reader thread");
}
