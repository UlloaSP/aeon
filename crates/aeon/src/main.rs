// Release builds are GUI-subsystem apps: launching aeon.exe must not flash a
// console window. Debug builds keep the console for panics and eprintln.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod font;
mod render;

use winit::event_loop::EventLoop;

/// The release version, stamped by CI through `AEON_VERSION` (nightlies carry
/// a suffix Cargo.toml does not). Local builds fall back to Cargo's version.
pub const VERSION: &str = match option_env!("AEON_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::<app::Event>::with_user_event().build()?;
    let mut app = app::App::new(event_loop.create_proxy());
    event_loop.run_app(&mut app)?;
    Ok(())
}
