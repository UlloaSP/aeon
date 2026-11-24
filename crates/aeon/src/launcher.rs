//! The resident Aeon (`aeon --background`): no terminal, a global hotkey, and
//! the launcher window it toggles. Started at sign-in by the MSI's Run entry.

use std::collections::{HashMap, HashSet};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use aeon_config::Config;
use aeon_launcher::profiles::{self, Kind};
use aeon_launcher::{
    Action, App, Hotkey, Icon, Icons, Item, Launcher, Profile, Sources, Theme as WindowsTheme,
    TrayCommand, apps, history, sys, update, updater,
};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::windows::{CornerPreference, WindowAttributesExtWindows};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Window, WindowId, WindowLevel};

use crate::chrome::Rect;
use crate::palette::{self, Content};
use crate::render::{Frame, Theme};
use crate::ui::{Palette, Ui};

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

const WIDTH: f64 = 640.0;

pub enum Event {
    Hotkey,
    /// The installed apps, from a background load.
    Catalog {
        apps: Vec<App>,
        /// Programs on PATH, which tell commands from web searches.
        commands: HashSet<String>,
        profiles: Vec<Profile>,
    },
    /// An app's icon, or `None` when the shell has none, from the icon thread.
    Icon(String, Option<Icon>),
    /// A click or menu pick on the notification-area icon.
    Tray(TrayCommand),
    /// The update thread's new status.
    Update(update::Status),
}

/// `check_updates`: a terminal asked to check while no resident ran.
pub fn run(at_sign_in: bool, check_updates: bool) -> Result<(), Box<dyn std::error::Error>> {
    if at_sign_in {
        let config = Config::default_path()
            .and_then(|p| Config::load(&p).ok())
            .unwrap_or_default();
        if !config.launcher.start_with_windows {
            return Ok(());
        }
    }
    if !sys::single_instance(INSTANCE_LOCK) {
        return Ok(());
    }
    let event_loop = EventLoop::<Event>::with_user_event().build()?;
    let mut resident = Resident::new(event_loop.create_proxy());
    if check_updates {
        resident.updater.request(updater::Request::Check);
    }
    event_loop.run_app(&mut resident)?;
    Ok(())
}

struct Resident {
    proxy: EventLoopProxy<Event>,
    /// Kept so the icon is removed when the resident exits.
    tray: Option<sys::Tray>,
    /// Holds the global hotkey; replaced when settings change it.
    hotkey_listener: Option<sys::HotkeyListener>,
    config: Config,
    colors: Palette,
    hotkey: Result<Hotkey, String>,
    view: Option<View>,
    launcher: Launcher,
    apps: Vec<App>,
    commands: HashSet<String>,
    profiles: Vec<Profile>,
    /// The mouse, for clicking a result.
    cursor: PhysicalPosition<f64>,
    /// When the app list was last requested; `None` before the first load.
    apps_requested: Option<Instant>,
    icons: Icons,
    /// Icons by app id, at the current scale. `None`: the shell has none.
    icon_cache: HashMap<String, Option<Icon>>,
    icons_pending: HashSet<String>,
    modifiers: ModifiersState,
    /// Shown in place of the placeholder: a bad hotkey or a failed launch.
    error: Option<String>,
    updater: updater::Updater,
    /// The last update the tray announced, so each is announced once.
    announced: Option<String>,
}

/// The window and what draws it, created once and then shown and hidden.
struct View {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    ui: Ui,
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
        let icons = {
            let proxy = proxy.clone();
            Icons::spawn(move |id, icon| {
                let _ = proxy.send_event(Event::Icon(id, icon));
            })
        };
        let updater = {
            let proxy = proxy.clone();
            updater::Updater::spawn(
                crate::VERSION,
                channel(&config),
                config.updates.check,
                move |status| {
                    let _ = proxy.send_event(Event::Update(status.clone()));
                },
            )
        };
        Self {
            updater,
            announced: None,
            proxy,
            tray: None,
            hotkey_listener: None,
            colors: Palette::new(&Theme::new(&config.colors)),
            config,
            hotkey,
            view: None,
            launcher: Launcher::default(),
            apps: Vec::new(),
            commands: HashSet::new(),
            profiles: Vec::new(),
            cursor: PhysicalPosition::new(-1.0, -1.0),
            apps_requested: None,
            icons,
            icon_cache: HashMap::new(),
            icons_pending: HashSet::new(),
            modifiers: ModifiersState::empty(),
            error: None,
        }
    }

    fn create_view(
        &self,
        event_loop: &ActiveEventLoop,
    ) -> Result<View, Box<dyn std::error::Error>> {
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
                    .with_inner_size(LogicalSize::new(WIDTH, 52.0)),
            )?,
        );
        let context = softbuffer::Context::new(window.clone())?;
        let surface = softbuffer::Surface::new(&context, window.clone())?;
        Ok(View {
            ui: Ui::new(window.scale_factor()),
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
        self.launcher = Launcher::open(Sources {
            history,
            apps: self.apps.clone(),
            commands: self.commands.clone(),
            search_url: self.config.launcher.search_url.clone(),
            profiles: self.profiles.clone(),
            ..Sources::default()
        });
        self.load_apps_if_stale();
        if self.hotkey.is_ok() {
            self.error = None;
        }
        self.show();
    }

    /// (Re)registers the configured hotkey. A resident nobody can reach is
    /// useless: when that fails, say why on screen.
    fn register_hotkey(&mut self) {
        // Only called at start and when the hotkey changed, so the old keys
        // and the new ones never collide while the old thread winds down.
        self.hotkey_listener = None;
        let registered = self.hotkey.clone().and_then(|hotkey| {
            let proxy = self.proxy.clone();
            sys::listen(hotkey, move || {
                let _ = proxy.send_event(Event::Hotkey);
            })
        });
        match registered {
            Ok(listener) => {
                self.hotkey_listener = Some(listener);
                self.error = None;
            }
            Err(e) => {
                self.error = Some(e);
                self.show();
            }
        }
    }

    /// A terminal saved settings: take the ones the launcher uses.
    fn reload(&mut self) {
        let config = Config::default_path()
            .and_then(|p| Config::load(&p).ok())
            .unwrap_or_default();
        let hotkey = config
            .launcher
            .hotkey
            .parse::<Hotkey>()
            .map_err(|e| format!("Invalid [launcher] hotkey: {e}"));
        self.colors = Palette::new(&Theme::new(&config.colors));
        self.updater.request(updater::Request::Configure {
            channel: channel(&config),
            automatic: config.updates.check,
        });
        let hotkey_changed = hotkey != self.hotkey;
        self.config = config;
        self.hotkey = hotkey;
        if hotkey_changed {
            self.register_hotkey();
        }
        if let Some(view) = &self.view {
            view.window.request_redraw();
        }
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
                let _ = proxy.send_event(Event::Catalog {
                    apps: apps::load(),
                    commands: apps::path_commands(),
                    profiles: profiles::detect(),
                });
            });
    }

    fn show(&mut self) {
        self.resize();
        let Some(view) = &self.view else { return };
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

    /// Asks for the icons of listed apps that have none yet.
    fn request_icons(&mut self) {
        let Some(view) = &self.view else { return };
        let size = view.ui.px(palette::ICON);
        for item in self.launcher.items() {
            if let Item::App(app) = item
                && !self.icon_cache.contains_key(&app.id)
                && self.icons_pending.insert(app.id.clone())
            {
                self.icons.request(&app.id, size);
            }
        }
    }

    /// Height follows the number of results.
    fn resize(&mut self) {
        self.request_icons();
        let Some(view) = &self.view else { return };
        let height = palette::height(&view.ui, self.launcher.items().len());
        let _ = view
            .window
            .request_inner_size(PhysicalSize::new(view.ui.px(WIDTH), height));
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
        let home = home_dir();
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
            Action::OpenUrl(url) => {
                if sys::open_url(url) {
                    Ok(())
                } else {
                    Err(std::io::Error::other("no browser opened the search"))
                }
            }
            Action::NewSession(profile) => open_profile(profile, &home),
            // The global palette lists no sessions or tasks.
            Action::GoToSession(_) | Action::RunTask(_) => Ok(()),
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
        let rect = Rect {
            x: 0,
            y: 0,
            w: frame.width,
            h: frame.height,
        };
        palette::draw(
            &mut frame,
            &mut view.ui,
            &self.colors,
            rect,
            &Content {
                launcher: &self.launcher,
                error: self.error.as_deref(),
                icons: Some(&self.icon_cache),
                in_window: false,
            },
        );
        let _ = buffer.present();
    }

    /// A click on a result chooses it.
    fn click(&mut self) {
        let Some(view) = &self.view else { return };
        let size = view.window.inner_size();
        let rect = Rect {
            x: 0,
            y: 0,
            w: size.width,
            h: size.height,
        };
        let rows = self.launcher.items().len();
        if let Some(row) = palette::row_at(&view.ui, rect, rows, self.cursor.x, self.cursor.y) {
            self.launcher.select(row);
            if let Some(action) = self.launcher.choose(self.modifiers.control_key()) {
                self.perform(action);
            }
        }
    }
}

fn channel(config: &Config) -> update::Channel {
    match config.updates.channel {
        aeon_config::Channel::Stable => update::Channel::Stable,
        aeon_config::Channel::Nightly => update::Channel::Nightly,
    }
}

/// Opens a terminal window for `profile` in `dir`; as administrator, through
/// the UAC prompt.
pub(crate) fn open_profile(profile: &Profile, dir: &Path) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    if profile.kind == Kind::Admin {
        // A trailing backslash would escape the closing quote.
        let mut dir_arg = dir.display().to_string();
        if dir_arg.ends_with('\\') {
            dir_arg.push('\\');
        }
        return if sys::run_as_admin(&exe, &format!("--cwd \"{dir_arg}\""), dir) {
            Ok(())
        } else {
            Err(std::io::Error::other(
                "Windows did not start it as administrator",
            ))
        };
    }
    let mut command = std::process::Command::new(exe);
    command.arg("--shell").arg(&profile.program);
    for arg in &profile.args {
        command.arg("--arg").arg(arg);
    }
    command.current_dir(dir).spawn().map(drop)
}

fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE").map_or_else(|| PathBuf::from("C:\\"), PathBuf::from)
}

/// Opens `config.toml` in the user's editor, creating it with a pointer to
/// the documentation when it does not exist yet: the menu item is the user
/// asking for it. Without an app for `.toml`, Notepad opens it.
pub(crate) fn open_settings() {
    let Some(path) = Config::default_path() else {
        return;
    };
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(
            &path,
            "# Aeon settings. Every key is optional, and an unknown key is an error.\n\
             # All options: https://github.com/UlloaSP/aeon#configuration\n",
        );
    }
    if !sys::open_url(&path.to_string_lossy()) {
        let _ = std::process::Command::new("notepad.exe").arg(&path).spawn();
    }
}

fn hwnd(window: &Window) -> Option<isize> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
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
        self.load_apps_if_stale();
        let proxy = self.proxy.clone();
        match sys::show_tray("Aeon", "Open launcher", move |command| {
            let _ = proxy.send_event(Event::Tray(command));
        }) {
            Ok(tray) => self.tray = Some(tray),
            Err(e) => eprintln!("aeon: no tray icon: {e}"),
        }
        self.register_hotkey();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        match event {
            Event::Hotkey => self.toggle(),
            Event::Tray(TrayCommand::OpenLauncher) => {
                if self.view.as_ref().and_then(|v| v.window.is_visible()) != Some(true) {
                    self.toggle();
                }
            }
            Event::Tray(TrayCommand::NewTerminal) => {
                let _ = std::env::current_exe().and_then(|exe| {
                    std::process::Command::new(exe)
                        .current_dir(home_dir())
                        .spawn()
                        .map(drop)
                });
            }
            Event::Tray(TrayCommand::OpenSettings) => open_settings(),
            Event::Tray(TrayCommand::Quit) => event_loop.exit(),
            Event::Tray(TrayCommand::Reload) => self.reload(),
            Event::Tray(TrayCommand::CheckUpdates) => self.updater.request(updater::Request::Check),
            Event::Tray(TrayCommand::DownloadUpdate) => {
                self.updater.request(updater::Request::Download)
            }
            Event::Tray(TrayCommand::InstallUpdate) => {
                self.updater.request(updater::Request::Install)
            }
            Event::Update(update::Status::Available(release)) => {
                if self.announced.as_ref() != Some(&release.version) {
                    if let Some(tray) = &self.tray {
                        tray.notify(
                            &format!("Aeon {} is available", release.version),
                            "Download it from the bottom of the sidebar in any Aeon window.",
                        );
                    }
                    self.announced = Some(release.version);
                }
            }
            Event::Update(_) => {}
            Event::Icon(id, icon) => {
                self.icons_pending.remove(&id);
                self.icon_cache.insert(id, icon);
                if let Some(view) = &self.view {
                    view.window.request_redraw();
                }
            }
            Event::Catalog {
                apps: mut list,
                commands,
                profiles,
            } => {
                self.commands = commands;
                self.profiles = profiles;
                if let Some(dir) = history::dir() {
                    apps::count_uses(&mut list, &history::app_uses(&dir));
                }
                self.apps = list;
                if self.view.as_ref().and_then(|v| v.window.is_visible()) == Some(true) {
                    self.launcher.set_profiles(self.profiles.clone());
                    self.launcher
                        .set_catalog(self.apps.clone(), self.commands.clone());
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
            WindowEvent::CursorMoved { position, .. } => self.cursor = position,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => self.click(),
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                // Icons were drawn for the old scale.
                self.icon_cache.clear();
                self.icons_pending.clear();
                if let Some(view) = &mut self.view {
                    view.ui.set_scale(scale_factor);
                    self.resize();
                }
            }
            _ => {}
        }
    }
}
