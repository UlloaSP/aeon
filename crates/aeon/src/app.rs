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
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{Window, WindowId};

use crate::font::{self, Atlas};
use crate::render::{self, Frame, Theme};

/// Sent from background threads to the UI thread.
pub enum Event {
    /// A chunk of shell output, from the PTY reader thread.
    Output(Vec<u8>),
    /// The shell exited.
    Exited,
    /// New context bar contents, from the context watcher thread.
    Context(Context),
}

pub struct App {
    proxy: EventLoopProxy<Event>,
    config: Config,
    config_error: Option<String>,
    theme: Theme,
    session: Option<Session>,
}

/// Everything that exists only while the window does.
struct Session {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    atlas: Atlas,
    term: Terminal,
    pty: Pty,
    input: File,
    context: Context,
    watcher: Option<Watcher>,
}

/// Where the grid and the context bar go in a window of a given size.
struct Layout {
    rows: usize,
    cols: usize,
    /// First pixel row of the context bar, when it is shown.
    bar_top: Option<u32>,
}

impl App {
    pub fn new(proxy: EventLoopProxy<Event>) -> Self {
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
            session: None,
        }
    }

    fn start(
        &mut self,
        event_loop: &ActiveEventLoop,
    ) -> Result<Session, Box<dyn std::error::Error>> {
        let window_cfg = &self.config.window;
        let pad = u32::from(window_cfg.padding);
        let font = font::load(self.config.font.path.as_deref())?;
        // The window starts at the default scale and is resized on the first
        // ScaleFactorChanged, so size it from 96-DPI metrics.
        let atlas = Atlas::new(font, points_to_px(self.config.font.size, 1.0));
        let bar_height = if self.config.bar.enabled {
            atlas.cell_height + pad
        } else {
            0
        };
        let inner = winit::dpi::LogicalSize::new(
            u32::from(window_cfg.columns) * atlas.cell_width + 2 * pad,
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
                    .with_inner_size(inner),
            )?,
        );
        let atlas = atlas.with_px(points_to_px(self.config.font.size, window.scale_factor()));

        let context = softbuffer::Context::new(window.clone())?;
        let surface = softbuffer::Surface::new(&context, window.clone())?;

        let layout = self.layout(window.inner_size(), &atlas, &window);
        let term = Terminal::new(layout.rows, layout.cols);

        let shell = self.shell_command();
        let mut pty = Pty::spawn(&shell, pty_size(layout.rows, layout.cols))?;
        let output = pty.take_reader().ok_or("pty reader already taken")?;
        let input = pty.take_writer().ok_or("pty writer already taken")?;
        spawn_reader(output, self.proxy.clone());

        let watcher = self.config.bar.enabled.then(|| {
            let proxy = self.proxy.clone();
            Watcher::spawn(pty.pid(), shell.cwd.clone(), move |context| {
                let _ = proxy.send_event(Event::Context(context));
            })
        });

        Ok(Session {
            window,
            surface,
            atlas,
            term,
            pty,
            input,
            context: Context {
                cwd: shell.cwd,
                ..Context::default()
            },
            watcher,
        })
    }

    fn shell_command(&self) -> aeon_pty::Command {
        let shell = &self.config.shell;
        let mut command = match &shell.program {
            Some(program) => aeon_pty::Command::new(program).args(&shell.args),
            None => aeon_pty::default_shell(),
        };
        if shell.integration {
            command = aeon_pty::integrate(command);
        }
        command.cwd = shell.cwd.clone().or_else(start_dir);
        command
            .env("TERM_PROGRAM", "aeon")
            .env("TERM_PROGRAM_VERSION", crate::VERSION)
            .env("COLORTERM", "truecolor")
    }

    fn scaled_padding(&self, window: &Window) -> u32 {
        (f64::from(self.config.window.padding) * window.scale_factor()).round() as u32
    }

    fn layout(&self, size: PhysicalSize<u32>, atlas: &Atlas, window: &Window) -> Layout {
        let padding = self.scaled_padding(window);
        let bar_height = if self.config.bar.enabled {
            atlas.cell_height + padding
        } else {
            0
        };
        let term_height = size.height.saturating_sub(bar_height);
        let w = size.width.saturating_sub(2 * padding);
        let h = term_height.saturating_sub(2 * padding);
        Layout {
            rows: (h / atlas.cell_height).max(1) as usize,
            cols: (w / atlas.cell_width).max(1) as usize,
            bar_top: self.config.bar.enabled.then_some(term_height),
        }
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let layout = self.layout(size, &session.atlas, &session.window);
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let (rows, cols) = (layout.rows, layout.cols);
        if (rows, cols) != (session.term.grid().rows(), session.term.grid().cols()) {
            session.term.resize(rows, cols);
            let _ = session.pty.resize(pty_size(rows, cols));
        }
        session.window.request_redraw();
    }

    fn redraw(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let padding = self.scaled_padding(&session.window);
        let size = session.window.inner_size();
        let layout = self.layout(size, &session.atlas, &session.window);
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        if session.surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buffer) = session.surface.buffer_mut() else {
            return;
        };
        let mut frame = Frame {
            pixels: &mut buffer,
            width: w.get(),
            height: h.get(),
        };
        render::draw(
            &mut frame,
            &session.term,
            &mut session.atlas,
            &self.theme,
            padding,
        );
        if let Some(top) = layout.bar_top {
            render::draw_bar(
                &mut frame,
                &mut session.atlas,
                &self.theme,
                &session.context,
                top,
                padding,
            );
        }
        let _ = buffer.present();
    }

    fn key(&mut self, event: &KeyEvent) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if event.state != ElementState::Pressed {
            return;
        }
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
}

impl ApplicationHandler<Event> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.session.is_some() {
            return;
        }
        match self.start(event_loop) {
            Ok(session) => self.session = Some(session),
            Err(e) => {
                eprintln!("aeon: failed to start: {e}");
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        match event {
            Event::Output(bytes) => {
                session.term.advance(&bytes);
                let mut redraw = session.term.take_dirty();
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
                    redraw = true;
                }
                if redraw {
                    session.window.request_redraw();
                }
            }
            Event::Context(context) => {
                // The watcher may answer for a directory the shell already
                // left; never let it overwrite a newer one.
                if context.cwd == session.context.cwd {
                    session.context = context;
                    session.window.request_redraw();
                }
            }
            Event::Exited => event_loop.exit(),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.resize(size),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(session) = self.session.take() {
                    let px = points_to_px(self.config.font.size, scale_factor);
                    let atlas = session.atlas.with_px(px);
                    let size = session.window.inner_size();
                    self.session = Some(Session { atlas, ..session });
                    self.resize(size);
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::KeyboardInput { event, .. } => self.key(&event),
            _ => {}
        }
    }
}

/// Launched from a shell, Aeon starts where that shell was. Launched from the
/// Start menu or Explorer, the working directory is the install or system
/// folder, which no one wants a prompt in, so start at home instead.
fn start_dir() -> Option<PathBuf> {
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

fn points_to_px(points: f32, scale_factor: f64) -> f32 {
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
fn spawn_reader(mut output: File, proxy: EventLoopProxy<Event>) {
    std::thread::Builder::new()
        .name("pty-reader".into())
        .spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match output.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if proxy.send_event(Event::Output(buf[..n].to_vec())).is_err() {
                            return;
                        }
                    }
                }
            }
            let _ = proxy.send_event(Event::Exited);
        })
        .expect("spawning the pty reader thread");
}
