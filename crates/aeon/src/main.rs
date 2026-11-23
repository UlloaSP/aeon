// Release builds are GUI-subsystem apps: launching aeon.exe must not flash a
// console window. Debug builds keep the console for panics and eprintln.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod font;
mod launcher;
mod render;

use winit::event_loop::EventLoop;

/// The release version, stamped by CI through `AEON_VERSION` (nightlies carry
/// a suffix Cargo.toml does not). Local builds fall back to Cargo's version.
pub const VERSION: &str = match option_env!("AEON_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// `aeon` opens a terminal. `aeon --command <cmd>` opens one that runs `cmd`
/// first. `aeon --background` is the resident launcher started at sign-in.
enum Mode {
    Terminal { run: Option<String> },
    Resident,
}

fn mode() -> Mode {
    let mut args = std::env::args().skip(1);
    let mut run = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--background" => return Mode::Resident,
            "--command" => run = args.next(),
            _ => {}
        }
    }
    Mode::Terminal { run }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match mode() {
        Mode::Resident => launcher::run(),
        Mode::Terminal { run } => {
            let event_loop = EventLoop::<app::Event>::with_user_event().build()?;
            let mut app = app::App::new(event_loop.create_proxy(), run);
            event_loop.run_app(&mut app)?;
            Ok(())
        }
    }
}
