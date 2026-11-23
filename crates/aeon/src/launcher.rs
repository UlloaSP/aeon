//! The resident Aeon (`aeon --background`): no terminal, a global hotkey, and
//! the launcher window it toggles. Started at sign-in by the MSI's Run entry.

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use aeon_config::Config;
use aeon_launcher::{
    Action, App, Hotkey, Item, Launcher, Theme as WindowsTheme, apps, history, sys,
};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::windows::{CornerPreference, WindowAttributesExtWindows};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Window, WindowId, WindowLevel};

use crate::app::points_to_px;
use crate::font::{self, Atlas};
use crate::render::{self, Frame, Theme};

/// One resident per sign-in session; a second `--background` just exits.
/// Development builds take their own lock, so they run beside an installed
/// Aeon instead of exiting.
const INSTANCE_LOCK: &str = if cfg!(debug_assertions) {
    "Local\\AeonResidentDev"
} else {
    "Local\\AeonResident"
};

/// Apps come and go rarely; the list is reloaded when the launcher opens and
/// the last load is older than this.
const APPS_MAX_AGE: Duration = Duration::from_secs(10 * 60);

const WIDTH: f64 = 680.0;
const INPUT_HEIGHT: f64 = 52.0;
const ROW_HEIGHT: f64 = 34.0;
const PAD: f64 = 8.0;
const INPUT_POINTS: f32 = 14.0;
const LIST_POINTS: f32 = 11.0;

pub enum Event {
    Hotkey,
    /// The installed apps, from a background load.
    Apps(Vec<App>),
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    if !sys::single_instance(INSTANCE_LOCK) {
        return Ok(());
    }
    let event_loop = EventLoop::<Event>::with_user_event().build()?;
    let mut resident = Resident::new(event_loop.create_proxy());
    event_loop.run_app(&mut resident)?;
    Ok(())
}

struct Resident {
    proxy: EventLoopProxy<Event>,
    config: Config,
    theme: Theme,
    hotkey: Result<Hotkey, String>,
    view: Option<View>,
    launcher: Launcher,
    apps: Vec<App>,
    /// When the app list was last requested; `None` before the first load.
    apps_requested: Option<Instant>,
    modifiers: ModifiersState,
    /// Shown in place of the placeholder: a bad hotkey or a failed launch.
    error: Option<String>,
}

/// The window and what draws it, created once and then shown and hidden.
struct View {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    input: Atlas,
    list: Atlas,
}

impl Resident {
    fn new(proxy: EventLoopProxy<Event>) -> Self {
        let config = Config::default_path()
            .and_then(|p| Config::load(&p).ok())
            .unwrap_or_default();
        let hotkey = config
            .launcher
            .hotkey
            .parse::<Hotkey>()
            .map_err(|e| format!("Invalid [launcher] hotkey: {e}"));
        Self {
            proxy,
            theme: Theme::new(&config.colors),
            config,
            hotkey,
            view: None,
            launcher: Launcher::default(),
            apps: Vec::new(),
            apps_requested: None,
            modifiers: ModifiersState::empty(),
            error: None,
        }
    }

    fn create_view(
        &self,
        event_loop: &ActiveEventLoop,
    ) -> Result<View, Box<dyn std::error::Error>> {
        let font = Rc::new(font::load(self.config.font.path.as_deref())?);
        let window = Rc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("Aeon")
                    .with_visible(false)
                    .with_decorations(false)
                    .with_resizable(false)
                    .with_window_level(WindowLevel::AlwaysOnTop)
                    .with_skip_taskbar(true)
                    .with_undecorated_shadow(true)
                    .with_corner_preference(CornerPreference::Round)
                    .with_inner_size(LogicalSize::new(WIDTH, INPUT_HEIGHT)),
            )?,
        );
        let scale = window.scale_factor();
        let context = softbuffer::Context::new(window.clone())?;
        let surface = softbuffer::Surface::new(&context, window.clone())?;
        Ok(View {
            input: Atlas::new(font.clone(), points_to_px(INPUT_POINTS, scale)),
            list: Atlas::new(font, points_to_px(LIST_POINTS, scale)),
            window,
            surface,
        })
    }

    fn toggle(&mut self) {
        let Some(view) = &self.view else { return };
        if view.window.is_visible() == Some(true) {
            self.hide();
            return;
        }
        // Reading history here is a few hundred kilobytes at most, once per
        // open; it keeps the launcher in step with the terminals' history.
        let history = history::dir()
            .map(|d| history::load(&d))
            .unwrap_or_default();
        if let Some(dir) = history::dir() {
            apps::count_uses(&mut self.apps, &history::app_uses(&dir));
        }
        self.launcher = Launcher::open(history, self.apps.clone());
        self.load_apps_if_stale();
        if self.hotkey.is_ok() {
            self.error = None;
        }
        self.show();
    }

    /// Loads the app list on a background thread when it is missing or old.
    fn load_apps_if_stale(&mut self) {
        if self
            .apps_requested
            .is_some_and(|t| t.elapsed() < APPS_MAX_AGE)
        {
            return;
        }
        self.apps_requested = Some(Instant::now());
        let proxy = self.proxy.clone();
        let _ = std::thread::Builder::new()
            .name("apps".into())
            .spawn(move || {
                let _ = proxy.send_event(Event::Apps(apps::load()));
            });
    }

    fn show(&mut self) {
        let Some(view) = &self.view else { return };
        self.resize();
        // Open on the monitor under the mouse, a quarter of the way down.
        let cursor = sys::cursor_position();
        let monitor = view
            .window
            .available_monitors()
            .find(|m| {
                let (p, s) = (m.position(), m.size());
                cursor.is_some_and(|(x, y)| {
                    x >= p.x && y >= p.y && x < p.x + s.width as i32 && y < p.y + s.height as i32
                })
            })
            .or_else(|| view.window.primary_monitor());
        if let Some(monitor) = monitor {
            let (p, s) = (monitor.position(), monitor.size());
            let width = (WIDTH * monitor.scale_factor()) as i32;
            view.window.set_outer_position(PhysicalPosition::new(
                p.x + (s.width as i32 - width) / 2,
                p.y + s.height as i32 / 4,
            ));
        }
        view.window.set_visible(true);
        view.window.focus_window();
        view.window.request_redraw();
    }

    fn hide(&mut self) {
        if let Some(view) = &self.view {
            view.window.set_visible(false);
        }
    }

    /// Height follows the number of results.
    fn resize(&self) {
        let Some(view) = &self.view else { return };
        let rows = self.launcher.items().len() as f64;
        let list = if rows > 0.0 {
            PAD + rows * ROW_HEIGHT + PAD
        } else {
            0.0
        };
        let _ = view
            .window
            .request_inner_size(LogicalSize::new(WIDTH, INPUT_HEIGHT + list));
        view.window.request_redraw();
    }

    fn key(&mut self, event: &KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }
        let ctrl = self.modifiers.control_key();
        let alt = self.modifiers.alt_key();
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => return self.hide(),
            Key::Named(NamedKey::Enter) => {
                if let Some(action) = self.launcher.choose(ctrl) {
                    self.perform(action);
                }
                return;
            }
            Key::Named(NamedKey::ArrowDown) => self.launcher.select_next(),
            Key::Named(NamedKey::ArrowUp) => self.launcher.select_previous(),
            Key::Named(NamedKey::Tab) => self.launcher.complete(),
            Key::Named(NamedKey::Backspace) if ctrl => self.launcher.delete_word(),
            Key::Named(NamedKey::Backspace) => self.launcher.backspace(),
            Key::Character(c) if ctrl && !alt && c.eq_ignore_ascii_case("v") => {
                if let Some(text) = sys::get_clipboard() {
                    self.launcher.insert(&text);
                }
            }
            // AltGr reports Ctrl+Alt, so typed text is taken whatever the
            // modifiers; control characters (Ctrl+letter) are dropped.
            _ => match &event.text {
                Some(text) if !text.chars().any(char::is_control) => self.launcher.insert(text),
                _ => return,
            },
        }
        self.error = None;
        self.resize();
    }

    fn perform(&mut self, action: Action) {
        let home =
            std::env::var_os("USERPROFILE").map_or_else(|| PathBuf::from("C:\\"), PathBuf::from);
        let result = match &action {
            Action::Copy(text) => match self.view.as_ref().and_then(|v| hwnd(&v.window)) {
                Some(owner) if sys::set_clipboard(owner, text) => Ok(()),
                _ => Err(std::io::Error::other("the clipboard is busy")),
            },
            Action::Run(command) => {
                let shell = aeon_pty::default_shell();
                aeon_launcher::run_detached(&shell.program, command, &home)
            }
            Action::RunInTerminal(command) => std::env::current_exe().and_then(|exe| {
                std::process::Command::new(exe)
                    .args(["--command", command])
                    .current_dir(&home)
                    .spawn()
                    .map(drop)
            }),
            Action::OpenApp(id) => apps::open(id),
            Action::SetTheme(theme) => {
                let light = match theme {
                    WindowsTheme::Toggle => !sys::light_theme().unwrap_or(false),
                    WindowsTheme::Dark => false,
                    WindowsTheme::Light => true,
                };
                // The change broadcast waits on every window; keep it off the
                // UI thread so the launcher closes at once.
                std::thread::spawn(move || sys::set_light_theme(light));
                Ok(())
            }
        };
        match result {
            Ok(()) => {
                match (&action, history::dir()) {
                    (Action::Run(command) | Action::RunInTerminal(command), Some(dir)) => {
                        let _ = history::append(&dir, command);
                    }
                    (Action::OpenApp(id), Some(dir)) => {
                        let _ = history::record_app(&dir, id);
                    }
                    _ => {}
                }
                self.hide();
            }
            Err(e) => {
                self.error = Some(format!("Could not start: {e}"));
                self.resize();
            }
        }
    }

    fn redraw(&mut self) {
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
        draw(
            &mut frame,
            &mut view.input,
            &mut view.list,
            &self.theme,
            &self.launcher,
            self.error.as_deref(),
            view.window.scale_factor(),
        );
        let _ = buffer.present();
    }
}

fn draw(
    frame: &mut Frame<'_>,
    input: &mut Atlas,
    list: &mut Atlas,
    theme: &Theme,
    launcher: &Launcher,
    error: Option<&str>,
    scale: f64,
) {
    let px = |logical: f64| (logical * scale).round() as u32;
    let [_, red, green, yellow, blue, magenta, ..] = theme.ansi;
    let muted = theme.ansi[8];
    frame.pixels.fill(theme.bar_bg);

    // Query row: accent chevron, text or placeholder, caret.
    let left = px(18.0);
    let text_y = (px(INPUT_HEIGHT).saturating_sub(input.cell_height) / 2) as i32;
    render::draw_glyph(frame, input, '\u{276F}', left as i32, text_y, theme.cursor);
    let text_x = left + input.cell_width * 2;
    let max_cols = (frame.width.saturating_sub(text_x + px(18.0)) / input.cell_width) as usize;
    let query = launcher.query();
    if let Some(error) = error {
        draw_text(frame, input, text_x, text_y, error, red, max_cols);
    } else if query.is_empty() {
        draw_text(
            frame,
            input,
            text_x,
            text_y,
            "Run a command, or do math",
            muted,
            max_cols,
        );
    } else {
        draw_text(frame, input, text_x, text_y, query, theme.fg, max_cols);
    }
    let caret_x = text_x + query.chars().count().min(max_cols) as u32 * input.cell_width;
    if error.is_none() {
        render::fill_rect(
            frame,
            caret_x,
            text_y as u32,
            px(2.0),
            input.cell_height,
            theme.cursor,
        );
    }

    let items = launcher.items();
    if items.is_empty() {
        return;
    }
    let divider = render::blend(theme.bar_bg, theme.fg, 24);
    render::fill_rect(
        frame,
        0,
        px(INPUT_HEIGHT),
        frame.width,
        px(1.0).max(1),
        divider,
    );

    let row_h = px(ROW_HEIGHT);
    let list_cols = (frame.width.saturating_sub(px(18.0) * 2) / list.cell_width) as usize;
    for (i, item) in items.iter().enumerate() {
        let top = px(INPUT_HEIGHT + PAD) + i as u32 * row_h;
        let selected = i == launcher.selected();
        if selected {
            let highlight = render::blend(theme.bar_bg, theme.fg, 22);
            render::fill_rect(
                frame,
                px(PAD),
                top,
                frame.width - 2 * px(PAD),
                row_h,
                highlight,
            );
        }
        let y = (top + row_h.saturating_sub(list.cell_height) / 2) as i32;
        let (icon, color) = match item {
            Item::Calc { .. } => ('=', yellow),
            Item::Run(_) => ('\u{276F}', green),
            Item::History(_) => ('\u{25F7}', muted),
            Item::App(_) => ('\u{25C6}', blue),
            Item::Builtin(_) => ('\u{25D0}', magenta),
        };
        render::draw_glyph(frame, list, icon, left as i32, y, color);

        let hint = match (item, selected) {
            (_, false) => "",
            (Item::Calc { .. }, true) => "Enter copies",
            (Item::App(_), true) => "Enter opens",
            (Item::Builtin(_), true) => "Enter applies",
            (_, true) => "Enter runs \u{b7} Ctrl+Enter in a terminal",
        };
        let hint_cols = hint.chars().count();
        let text_cols = list_cols.saturating_sub(3 + if hint_cols > 0 { hint_cols + 2 } else { 0 });
        draw_text(
            frame,
            list,
            left + list.cell_width * 3,
            y,
            item.text(),
            theme.fg,
            text_cols,
        );
        if hint_cols > 0 {
            let hint_x = frame.width - px(18.0) - hint_cols as u32 * list.cell_width;
            draw_text(frame, list, hint_x, y, hint, muted, hint_cols);
        }
    }
}

fn hwnd(window: &Window) -> Option<isize> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

/// Monospace text from `x`, cut with an ellipsis past `max_cols`.
fn draw_text(
    frame: &mut Frame<'_>,
    atlas: &mut Atlas,
    x: u32,
    y: i32,
    text: &str,
    color: u32,
    max_cols: usize,
) {
    let count = text.chars().count();
    let shown: Box<dyn Iterator<Item = char>> = if count > max_cols && max_cols > 0 {
        Box::new(text.chars().take(max_cols - 1).chain(Some('\u{2026}')))
    } else {
        Box::new(text.chars().take(max_cols))
    };
    for (i, ch) in shown.enumerate() {
        if ch != ' ' {
            let cx = x + i as u32 * atlas.cell_width;
            render::draw_glyph(frame, atlas, ch, cx as i32, y, color);
        }
    }
}

impl ApplicationHandler<Event> for Resident {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.view.is_some() {
            return;
        }
        match self.create_view(event_loop) {
            Ok(view) => self.view = Some(view),
            Err(e) => {
                eprintln!("aeon: launcher failed to start: {e}");
                event_loop.exit();
                return;
            }
        }
        let registered = self.hotkey.clone().and_then(|hotkey| {
            let proxy = self.proxy.clone();
            sys::listen(hotkey, move || {
                let _ = proxy.send_event(Event::Hotkey);
            })
        });
        self.load_apps_if_stale();
        // A resident nobody can reach is useless: say why, once, on screen.
        if let Err(e) = registered {
            self.error = Some(e);
            self.show();
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: Event) {
        match event {
            Event::Hotkey => self.toggle(),
            Event::Apps(mut list) => {
                if let Some(dir) = history::dir() {
                    apps::count_uses(&mut list, &history::app_uses(&dir));
                }
                self.apps = list;
                if self.view.as_ref().and_then(|v| v.window.is_visible()) == Some(true) {
                    self.launcher.set_apps(self.apps.clone());
                    self.resize();
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            // Only the system closes this window (sign-out, the installer):
            // that ends the resident. Escape and focus loss only hide it.
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Focused(false) => self.hide(),
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } => self.key(&event),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(view) = self.view.take() {
                    self.view = Some(View {
                        input: view.input.with_px(points_to_px(INPUT_POINTS, scale_factor)),
                        list: view.list.with_px(points_to_px(LIST_POINTS, scale_factor)),
                        ..view
                    });
                    self.resize();
                }
            }
            _ => {}
        }
    }
}
