use std::fs::File;
use std::io::{Read, Write};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;

use aeon_config::Config;
use aeon_context::{Context, Watcher};
use aeon_pty::{Pty, PtySize};
use aeon_term::Terminal;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::platform::windows::IconExtWindows;
use winit::window::{CursorIcon, Icon, Window, WindowId};

use crate::font::{self, Atlas};
use crate::keys::Combo;
use crate::render::{self, Frame, Sidebar, SidebarEntry, Theme};
use crate::settings::{self, Outcome, Settings};

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
}

/// The window and what draws into it; shared by every session.
struct View {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    atlas: Atlas,
}

/// One shell: its PTY, what it printed, and what the bar and sidebar show.
struct Session {
    id: SessionId,
    term: Terminal,
    pty: Pty,
    input: File,
    context: Context,
    watcher: Option<Watcher>,
}

/// Where everything goes in a window of a given size.
struct Layout {
    /// Width of the sidebar; 0 when it is hidden.
    sidebar: u32,
    /// Top-left pixel of the grid.
    origin: (u32, u32),
    rows: usize,
    cols: usize,
    /// First pixel row of the context bar, when it is shown.
    bar_top: Option<u32>,
}

impl App {
    pub fn new(proxy: EventLoopProxy<Event>, run: Option<String>) -> Self {
        let (config, config_error) = match Config::default_path().map(|p| Config::load(&p)) {
            Some(Ok(config)) => (config, None),
            Some(Err(e)) => (Config::default(), Some(e.to_string())),
            None => (Config::default(), None),
        };
        Self {
            proxy,
            theme: Theme::new(&config.colors),
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
        let bar_height = if self.config.bar.enabled {
            atlas.cell_height + pad
        } else {
            0
        };
        let sidebar = if self.sidebar_visible {
            Sidebar::new(&atlas, 1.0).width
        } else {
            0
        };
        let inner = winit::dpi::LogicalSize::new(
            sidebar + u32::from(window_cfg.columns) * atlas.cell_width + 2 * pad,
            u32::from(window_cfg.rows) * atlas.cell_height + 2 * pad + bar_height,
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
                    .with_inner_size(inner),
            )?,
        );
        let atlas = atlas.with_px(points_to_px(self.config.font.size, window.scale_factor()));
        let context = softbuffer::Context::new(window.clone())?;
        let surface = softbuffer::Surface::new(&context, window.clone())?;
        Ok(View {
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
            term: Terminal::new(layout.rows, layout.cols),
            pty,
            input,
            context: Context {
                cwd: shell.cwd,
                ..Context::default()
            },
            watcher,
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
            Sidebar::new(atlas, scale).width.min(size.width / 2)
        } else {
            0
        };
        let bar_height = if self.config.bar.enabled {
            atlas.cell_height + padding
        } else {
            0
        };
        let term_height = size.height.saturating_sub(bar_height);
        let w = size.width.saturating_sub(sidebar + 2 * padding);
        let h = term_height.saturating_sub(2 * padding);
        Layout {
            sidebar,
            origin: (sidebar + padding, padding),
            rows: (h / atlas.cell_height).max(1) as usize,
            cols: (w / atlas.cell_width).max(1) as usize,
            bar_top: self.config.bar.enabled.then_some(term_height),
        }
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
        let padding = (f64::from(self.config.window.padding) * scale).round() as u32;
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
            frame.pixels.fill(self.theme.bg);
            let geo = settings::Geometry::new(layout.sidebar, frame.width, &view.atlas, scale);
            panel.draw(&mut frame, &mut view.atlas, &self.theme, &geo);
        } else {
            render::draw(
                &mut frame,
                &session.term,
                &mut view.atlas,
                &self.theme,
                layout.origin,
                self.config.cursor.style,
            );
        }
        if let Some(top) = layout.bar_top.filter(|_| self.settings.is_none()) {
            render::draw_bar(
                &mut frame,
                &mut view.atlas,
                &self.theme,
                &session.context,
                (layout.sidebar, top),
                padding,
            );
        }
        if layout.sidebar > 0 {
            let entries: Vec<SidebarEntry<'_>> = self
                .sessions
                .iter()
                .map(|s| SidebarEntry {
                    context: &s.context,
                })
                .collect();
            let sidebar = Sidebar::new(&view.atlas, scale);
            render::draw_sidebar(
                &mut frame,
                &mut view.atlas,
                &self.theme,
                &sidebar,
                &entries,
                self.active,
                self.settings.is_some(),
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
        ]
        .into_iter()
        .find(|(text, _)| text.parse::<Combo>().is_ok_and(|c| c.matches(key, mods)))
        .map(|(_, action)| action);
        let n = self.sessions.len();
        match action {
            Some(Action::New) => self.new_session(),
            Some(Action::Close) => self.close_session(self.active, event_loop),
            Some(Action::Next) if n > 1 => self.select((self.active + 1) % n),
            Some(Action::Previous) if n > 1 => self.select((self.active + n - 1) % n),
            Some(Action::Next | Action::Previous) => {}
            Some(Action::Sidebar) => self.toggle_sidebar(),
            Some(Action::Settings) => self.toggle_settings(),
            None => {
                let digit = match key {
                    Key::Character(c) if mods == ModifiersState::CONTROL => c.parse::<usize>().ok(),
                    _ => None,
                };
                match digit {
                    Some(d @ 1..=9) => self.select(d - 1),
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
        let Some(session) = self.sessions.get_mut(self.active) else {
            return;
        };
        let bytes: &[u8] = match &event.logical_key {
            Key::Named(NamedKey::Enter) => b"\r",
            Key::Named(NamedKey::Backspace) => b"\x7f",
            Key::Named(NamedKey::Tab) => b"\t",
            Key::Named(NamedKey::Escape) => b"\x1b",
            Key::Named(NamedKey::ArrowUp) => b"\x1b[A",
            Key::Named(NamedKey::ArrowDown) => b"\x1b[B",
            Key::Named(NamedKey::ArrowRight) => b"\x1b[C",
            Key::Named(NamedKey::ArrowLeft) => b"\x1b[D",
            Key::Named(NamedKey::Home) => b"\x1b[H",
            Key::Named(NamedKey::End) => b"\x1b[F",
            Key::Named(NamedKey::Delete) => b"\x1b[3~",
            // Includes Ctrl+letter as its control character (Ctrl+C is 0x03).
            _ => match event.text_with_all_modifiers() {
                Some(text) => text.as_bytes(),
                None => return,
            },
        };
        // ConPTY drains its input pipe continuously; a keystroke-sized write
        // does not block the UI thread in practice.
        let _ = session.input.write_all(bytes);
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
                let geo = settings::Geometry::new(layout.sidebar, size.width, &view.atlas, scale);
                let outcome = panel.click(x, y, &geo, &self.config);
                self.settings_outcome(outcome);
            }
            return;
        }
        let sidebar = Sidebar::new(&view.atlas, scale);
        if sidebar.is_settings_button(x, y, size.height) {
            self.toggle_settings();
        } else if sidebar.is_new_button(x, y, &view.atlas) {
            self.new_session();
        } else if let Some(i) = sidebar.row_at(x, y, self.sessions.len()) {
            self.settings = None;
            self.select(i);
            self.request_redraw();
        }
    }

    /// Shows a hand over anything a click acts on, the arrow elsewhere.
    fn update_pointer(&mut self) {
        let Some(view) = &self.view else {
            return;
        };
        let (x, y) = (self.cursor.x, self.cursor.y);
        let layout = self.layout(view);
        let scale = view.window.scale_factor();
        let size = view.window.inner_size();
        let clickable = if x < f64::from(layout.sidebar) {
            let sidebar = Sidebar::new(&view.atlas, scale);
            sidebar.is_settings_button(x, y, size.height)
                || sidebar.is_new_button(x, y, &view.atlas)
                || sidebar.row_at(x, y, self.sessions.len()).is_some()
        } else {
            self.settings.as_ref().is_some_and(|panel| {
                let geo = settings::Geometry::new(layout.sidebar, size.width, &view.atlas, scale);
                panel.is_clickable(x, y, &geo)
            })
        };
        if clickable != self.pointer {
            self.pointer = clickable;
            view.window.set_cursor(if clickable {
                CursorIcon::Pointer
            } else {
                CursorIcon::Default
            });
        }
    }

    fn toggle_settings(&mut self) {
        self.settings = match self.settings {
            Some(_) => None,
            None => Some(Settings::new(&self.config)),
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
            let px = points_to_px(config.font.size, view.window.scale_factor());
            view.atlas = Atlas::new(Rc::new(font), px);
        }
        self.theme = Theme::new(&config.colors);
        self.config = config;
        self.resize();
        aeon_launcher::sys::notify_settings_changed();
        Ok(())
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

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        match event {
            Event::Output(id, bytes) => {
                let Some(index) = self.session_index(id) else {
                    return;
                };
                let session = &mut self.sessions[index];
                session.term.advance(&bytes);
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
                if let Some(view) = self.view.take() {
                    let px = points_to_px(self.config.font.size, scale_factor);
                    self.view = Some(View {
                        atlas: view.atlas.with_px(px),
                        ..view
                    });
                    self.resize();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } => self.key(&event, event_loop),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = position;
                self.update_pointer();
            }
            WindowEvent::CursorLeft { .. } => self.cursor = PhysicalPosition::new(-1.0, -1.0),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => self.click(),
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
