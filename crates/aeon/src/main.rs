// Release builds are GUI-subsystem apps: launching aeon.exe must not flash a
// console window. Debug builds keep the console for panics and eprintln.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod chrome;
mod font;
mod input;
mod keys;
mod launcher;
mod links;
mod palette;
mod panes;
mod render;
mod settings;
mod sys;
mod ui;

use winit::event_loop::EventLoop;

/// The release version, stamped by CI through `AEON_VERSION` (nightlies carry
/// a suffix Cargo.toml does not). Local builds fall back to Cargo's version.
pub const VERSION: &str = match option_env!("AEON_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// `aeon` opens a terminal. `aeon --command <cmd>` opens one that runs `cmd`
/// first; `--shell <program>` with any number of `--arg <arg>` runs that
/// shell, and `--cwd <dir>` starts there (the palette's new-session profiles
/// use these). `aeon --background` is the resident launcher; the installer's
/// startup entry adds `--at-sign-in`, so the resident can honor the
/// "Start with Windows" setting, and a terminal that asks for an update check
/// while no resident runs starts one with `--check-updates`.
enum Mode {
    Terminal(app::Launch),
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
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1).cloned())
    };
    let program = value("--shell").map(|program| app::Program {
        program,
        args: args
            .windows(2)
            .filter(|pair| pair[0] == "--arg")
            .map(|pair| pair[1].clone())
            .collect(),
    });
    Mode::Terminal(app::Launch {
        run: value("--command"),
        program,
        cwd: value("--cwd").map(std::path::PathBuf::from),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match mode() {
        Mode::Resident {
            at_sign_in,
            check_updates,
        } => launcher::run(at_sign_in, check_updates),
        Mode::Terminal(launch) => {
            let event_loop = EventLoop::<app::Event>::with_user_event().build()?;
            let mut app = app::App::new(event_loop.create_proxy(), launch);
            event_loop.run_app(&mut app)?;
            Ok(())
        }
    }
}
