use std::fs::File;
use std::io::{Read, Write};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use aeon_config::Config;
use aeon_context::{Context, Watcher};
use aeon_launcher::ResidentCommand;
use aeon_launcher::update::Status;
use aeon_launcher::{sys, updater};
use aeon_pty::{Pty, PtySize};
use aeon_term::{Match, MouseMode, Point, Terminal};
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

use crate::chrome::{self, Hit, Pill, Project, SessionRow, Sidebar, SidebarState};
use crate::font::{self, Atlas};
use crate::input::{self, Button, MouseAction};
use crate::keys::Combo;
use crate::links::{self, Target};
use crate::render::{self, Frame, Overlay, Theme};
use crate::settings::{self, Outcome, Settings};
use crate::ui::{Palette, Ui};

/// Identifies a session across threads; never reused within a process.
type SessionId = u64;

/// Sent from background threads to the UI thread.
pub enum Event {
    /// A chunk of shell output, from a session's PTY reader thread.
    Output(SessionId, Vec<u8>),
    /// A session's shell exited.
    Exited(SessionId),
    /// New context bar contents, from a session's watcher thread.
    Context(SessionId, Context),
}

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
    /// A first command for the first shell, from `--command` (the launcher's
    /// Ctrl+Enter).
    run: Option<String>,
    view: Option<View>,
    sessions: Vec<Session>,
    active: usize,
    next_id: SessionId,
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
}

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
}

/// Where everything goes in a window of a given size.
struct Layout {
    /// Width of the sidebar; 0 when it is hidden.
    sidebar: u32,
    /// Top-left pixel of the grid.
    origin: (u32, u32),
    rows: usize,
    cols: usize,
    /// Height of the session header above the terminal; 0 when it is off.
    header: u32,
}

/// The sidebar's width in logical pixels, as in T3 Code.
const SIDEBAR_WIDTH: f64 = 256.0;
/// The session header's height in logical pixels.
const HEADER_HEIGHT: f64 = 44.0;

impl App {
    pub fn new(proxy: EventLoopProxy<Event>, run: Option<String>) -> Self {
        let (config, config_error) = match Config::default_path().map(|p| Config::load(&p)) {
            Some(Ok(config)) => (config, None),
            Some(Err(e)) => (Config::default(), Some(e.to_string())),
            None => (Config::default(), None),
        };
        let theme = Theme::new(&config.colors);
        Self {
            proxy,
            palette: Palette::new(&theme),
            theme,
            home: std::env::var_os("USERPROFILE").map(PathBuf::from),
            hover: None,
            config,
            config_error,
            run,
            view: None,
            sessions: Vec::new(),
            active: 0,
            next_id: 0,
            sidebar_visible: state::sidebar_visible(),
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

    /// Starts a shell in `cwd` and makes it the active session.
    fn open_session(
        &mut self,
        cwd: Option<PathBuf>,
        run: Option<String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let Some(view) = &self.view else {
            return Err("no window".into());
        };
        let layout = self.layout(view);
        let id = self.next_id;
        self.next_id += 1;

        let shell = self.shell_command(cwd, run.as_deref());
        let shell_name = std::path::Path::new(&shell.program)
            .file_stem()
            .map_or_else(
                || "shell".to_owned(),
                |n| n.to_string_lossy().to_lowercase(),
            );
        let mut pty = Pty::spawn(&shell, pty_size(layout.rows, layout.cols))?;
        let output = pty.take_reader().ok_or("pty reader already taken")?;
        let input = pty.take_writer().ok_or("pty writer already taken")?;
        spawn_reader(id, output, self.proxy.clone());

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
            term: Terminal::with_scrollback(
                layout.rows,
                layout.cols,
                self.config.terminal.scrollback,
            ),
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
        });
        self.active = self.sessions.len() - 1;
        self.update_title();
        self.request_redraw();
        Ok(())
    }

    fn shell_command(&self, cwd: Option<PathBuf>, run: Option<&str>) -> aeon_pty::Command {
        let shell = &self.config.shell;
        let mut command = match &shell.program {
            Some(program) => aeon_pty::Command::new(program).args(&shell.args),
            None => aeon_pty::default_shell(),
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

    fn close_session(&mut self, index: usize, event_loop: &ActiveEventLoop) {
        if index >= self.sessions.len() {
            return;
        }
        // Dropping the session closes its pseudo console, which ends the shell.
        self.sessions.remove(index);
        if self.sessions.is_empty() {
            event_loop.exit();
            return;
        }
        if self.active > index || self.active >= self.sessions.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.update_title();
        self.request_redraw();
    }

    fn select(&mut self, index: usize) {
        if index < self.sessions.len() && index != self.active {
            self.active = index;
            self.update_title();
            self.request_redraw();
        }
    }

    fn toggle_sidebar(&mut self) {
        self.sidebar_visible = !self.sidebar_visible;
        state::set_sidebar_visible(self.sidebar_visible);
        self.resize();
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
        let scale = view.window.scale_factor();
        let padding = (f64::from(self.config.window.padding) * scale).round() as u32;
        let atlas = &view.atlas;
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
        let w = size.width.saturating_sub(sidebar + 2 * padding);
        let h = size.height.saturating_sub(header + 2 * padding);
        Layout {
            sidebar,
            origin: (sidebar + padding, header + padding),
            rows: (h / atlas.cell_height).max(1) as usize,
            cols: (w / atlas.cell_width).max(1) as usize,
            header,
        }
    }

    /// Sessions grouped into projects, as the sidebar shows them.
    fn projects(&self) -> Vec<Project> {
        let contexts: Vec<&Context> = self.sessions.iter().map(|s| &s.context).collect();
        chrome::projects(&contexts, self.home.as_deref())
    }

    /// Moves `step` sessions along the sidebar's order, wrapping around.
    fn step(&mut self, step: isize) {
        let order = chrome::display_order(&self.projects());
        let Some(at) = order.iter().position(|&i| i == self.active) else {
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

    /// Fits every session's grid and PTY to the window.
    fn resize(&mut self) {
        let Some(view) = &self.view else {
            return;
        };
        let layout = self.layout(view);
        let (rows, cols) = (layout.rows, layout.cols);
        for session in &mut self.sessions {
            if (rows, cols) != (session.term.grid().rows(), session.term.grid().cols()) {
                session.term.resize(rows, cols);
                let _ = session.pty.resize(pty_size(rows, cols));
            }
        }
        self.request_redraw();
    }

    fn redraw(&mut self) {
        let Some(view) = self.view.as_ref() else {
            return;
        };
        let layout = self.layout(view);
        let scale = view.window.scale_factor();
        let projects = self.projects();
        let pill = self.pill();
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
        let home = self.home.as_deref();
        let title = |i: usize, s: &Session| {
            projects
                .iter()
                .find(|p| p.sessions.contains(&i))
                .map_or_else(
                    || s.shell.clone(),
                    |p| chrome::session_title(&s.context, &s.shell, p, home),
                )
        };
        if let Some(panel) = &self.settings {
            frame.pixels.fill(self.palette.canvas);
            let geo = settings::Geometry::new(layout.sidebar, frame.width, scale);
            panel.draw(&mut frame, &mut view.ui, &self.palette, &geo);
        } else {
            let overlay = Overlay {
                scroll: session.scroll,
                selection: session.selection.and_then(|s| s.ordered()),
                matches: self.find.as_ref().map_or(&[], |f| &f.matches[..]),
                current: self.find.as_ref().and_then(|f| f.current),
                link: self.link.as_ref().map(|(line, a, b, _)| (*line, *a, *b)),
            };
            render::draw(
                &mut frame,
                &session.term,
                &mut view.atlas,
                &self.theme,
                layout.origin,
                self.config.cursor.style,
                &overlay,
            );
            if layout.header > 0
                && let Some(project) = projects.iter().find(|p| p.sessions.contains(&self.active))
            {
                chrome::draw_header(
                    &mut frame,
                    &mut view.ui,
                    &self.palette,
                    layout.sidebar,
                    project,
                    &title(self.active, session),
                    &session.context,
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
            let rows: Vec<SessionRow<'_>> = self
                .sessions
                .iter()
                .enumerate()
                .map(|(i, s)| SessionRow {
                    context: &s.context,
                    title: title(i, s),
                })
                .collect();
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
                    active: self.active,
                    settings_open: self.settings.is_some(),
                    hover: self.hover,
                    pill: pill.as_ref(),
                    new_keys: &self.config.keys.new_session,
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
        ]
        .into_iter()
        .find(|(text, _)| text.parse::<Combo>().is_ok_and(|c| c.matches(key, mods)))
        .map(|(_, action)| action);
        match action {
            Some(Action::New) => self.new_session(),
            Some(Action::Close) => self.close_session(self.active, event_loop),
            Some(Action::Next) => self.step(1),
            Some(Action::Previous) => self.step(-1),
            Some(Action::Sidebar) => self.toggle_sidebar(),
            Some(Action::Settings) => self.toggle_settings(),
            Some(Action::Find) => self.open_find(),
            Some(Action::Copy) => {
                self.copy();
            }
            Some(Action::Paste) => self.paste(),
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

    /// The terminal cell under (`x`, `y`): column and screen row, clamped to
    /// the grid. `None` outside the terminal.
    fn cell_at(&self, x: f64, y: f64) -> Option<(usize, usize)> {
        let view = self.view.as_ref()?;
        if self.settings.is_some() {
            return None;
        }
        let layout = self.layout(view);
        let (left, top) = layout.origin;
        if x < f64::from(layout.sidebar) || y < f64::from(layout.header) {
            return None;
        }
        let session = self.sessions.get(self.active)?;
        let grid = session.term.grid();
        let col = ((x - f64::from(left)).max(0.0) / f64::from(view.atlas.cell_width)) as usize;
        let row = ((y - f64::from(top)).max(0.0) / f64::from(view.atlas.cell_height)) as usize;
        Some((col.min(grid.cols() - 1), row.min(grid.rows() - 1)))
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
        let Some(view) = &self.view else {
            return;
        };
        let layout = self.layout(view);
        let scale = view.window.scale_factor();
        let size = view.window.inner_size();
        let clickable = hover.is_some()
            || (x >= f64::from(layout.sidebar)
                && self.settings.as_ref().is_some_and(|panel| {
                    let geo = settings::Geometry::new(layout.sidebar, size.width, scale);
                    panel.is_clickable(x, y, &geo)
                }));
        let in_terminal = self.cell_at(x, y).is_some();
        self.hover_link();
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
            let run = self.run.take();
            self.open_session(None, run)
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
            Event::Output(id, bytes) => {
                let Some(index) = self.session_index(id) else {
                    return;
                };
                let session = &mut self.sessions[index];
                session.term.advance(&bytes);
                let scrolled = session.term.take_scrolled();
                if session.term.modes().alt_screen {
                    session.scroll = 0;
                } else if session.scroll > 0 {
                    // Stay on the same text while new lines arrive below.
                    session.scroll = (session.scroll + scrolled).min(session.term.scrollback_len());
                }
                let mut redraw = session.term.take_dirty() && index == self.active;
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
                    self.close_session(index, event_loop);
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
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
            WindowEvent::Focused(true) => self.refresh_update(),
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

    fn path() -> Option<PathBuf> {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("aeon").join("state"))
    }

    pub fn sidebar_visible() -> bool {
        let text = path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        !text.lines().any(|l| l.trim() == "sidebar=hidden")
    }

    pub fn set_sidebar_visible(visible: bool) {
        let Some(path) = path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let value = if visible { "shown" } else { "hidden" };
        let _ = std::fs::write(path, format!("sidebar={value}\n"));
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
