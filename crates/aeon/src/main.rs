// Release builds are GUI-subsystem apps: launching aeon.exe must not flash a
// console window. Debug builds keep the console for panics and eprintln.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod font;
mod keys;
mod launcher;
mod render;
mod settings;

use winit::event_loop::EventLoop;

/// The release version, stamped by CI through `AEON_VERSION` (nightlies carry
/// a suffix Cargo.toml does not). Local builds fall back to Cargo's version.
pub const VERSION: &str = match option_env!("AEON_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// `aeon` opens a terminal. `aeon --command <cmd>` opens one that runs `cmd`
/// first. `aeon --background` is the resident launcher; the installer's
/// startup entry adds `--at-sign-in`, so the resident can honor the
/// "Start with Windows" setting, and a terminal that asks for an update check
/// while no resident runs starts one with `--check-updates`.
enum Mode {
    Terminal {
        run: Option<String>,
    },
    Resident {
        at_sign_in: bool,
        check_updates: bool,
    },
}

fn mode() -> Mode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--background") {
        return Mode::Resident {
            at_sign_in: args.iter().any(|a| a == "--at-sign-in"),
            check_updates: args.iter().any(|a| a == "--check-updates"),
        };
    }
    let run = args
        .iter()
        .position(|a| a == "--command")
        .and_then(|i| args.get(i + 1).cloned());
    Mode::Terminal { run }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match mode() {
        Mode::Resident {
            at_sign_in,
            check_updates,
        } => launcher::run(at_sign_in, check_updates),
        Mode::Terminal { run } => {
            let event_loop = EventLoop::<app::Event>::with_user_event().build()?;
            let mut app = app::App::new(event_loop.create_proxy(), run);
            event_loop.run_app(&mut app)?;
            Ok(())
        }
    }
}
