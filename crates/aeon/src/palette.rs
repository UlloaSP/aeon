//! The palette as both places show it: the launcher window behind the global
//! hotkey, and the box over a terminal window. Its state is
//! `aeon_launcher::Launcher`; this only draws it.

use std::collections::HashMap;

use aeon_launcher::profiles::Kind;
use aeon_launcher::{Icon, Item, Launcher};

use crate::chrome::Rect;
use crate::render::{Frame, blend, fill_rect};
use crate::sys::Face;
use crate::ui::{self, Palette, REGULAR, Ui, icon};

const INPUT: f64 = 52.0;
const ROW: f64 = 40.0;
const PAD: f64 = 6.0;
/// App icons in the list, in logical pixels.
pub const ICON: f64 = 24.0;

/// What one frame of the palette shows.
pub struct Content<'a> {
    pub launcher: &'a Launcher,
    /// Shown in place of the placeholder: a bad hotkey or a failed launch.
    pub error: Option<&'a str>,
    /// App icons by id; `None` where there are none to show.
    pub icons: Option<&'a HashMap<String, Option<Icon>>>,
    /// Over a terminal window: Ctrl+Enter runs in a new session of it, and
    /// the box draws its own border.
    pub in_window: bool,
}

/// The palette's height for `rows` results.
pub fn height(ui: &Ui, rows: usize) -> u32 {
    let list = if rows > 0 {
        ui.px(PAD) * 2 + rows as u32 * ui.px(ROW) + 1
    } else {
        0
    };
    ui.px(INPUT) + list
}

/// The result row under (`x`, `y`).
pub fn row_at(ui: &Ui, rect: Rect, rows: usize, x: f64, y: f64) -> Option<usize> {
    if !rect.contains(x, y) {
        return None;
    }
    let top = f64::from(rect.y + ui.px(INPUT) + 1 + ui.px(PAD));
    let row = ((y - top) / f64::from(ui.px(ROW))).floor();
    (row >= 0.0 && (row as usize) < rows).then_some(row as usize)
}

pub fn draw(frame: &mut Frame<'_>, ui: &mut Ui, pal: &Palette, rect: Rect, content: &Content<'_>) {
    let launcher = content.launcher;
    if content.in_window {
        ui::card(
            frame,
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            ui.px(10.0),
            pal.raised,
            blend(pal.canvas, pal.fg, 44),
        );
    } else {
        fill_rect(frame, rect.x, rect.y, rect.w, rect.h, pal.raised);
    }
    let body = ui.font(Face::Ui, 14.0, REGULAR);
    let query_font = ui.font(Face::Ui, 15.0, REGULAR);
    let small = ui.font(Face::Ui, 12.0, REGULAR);

    // The query: an icon, the text or a placeholder, and the caret.
    let mid = rect.y + ui.px(INPUT) / 2;
    let lead = if launcher.is_menu() {
        icon::ADD
    } else {
        icon::SEARCH
    };
    ui.icon(frame, rect.x + ui.px(22.0), mid, lead, 13.0, pal.label);
    let tx = rect.x + ui.px(44.0);
    let room = (rect.x + rect.w).saturating_sub(tx + ui.px(18.0));
    let query = launcher.query();
    if let Some(error) = content.error {
        ui.text_mid(frame, tx, mid, error, query_font, pal.red, room);
    } else {
        let drawn = if query.is_empty() {
            let placeholder = match (launcher.is_menu(), content.in_window) {
                (true, _) => "New session with\u{2026}",
                (false, true) => "Go to a session, run a task or command, do math",
                (false, false) => "Run a command, open an app, do math",
            };
            ui.text_mid(frame, tx, mid, placeholder, query_font, pal.label, room);
            0
        } else {
            ui.text_mid(frame, tx, mid, query, query_font, pal.fg, room)
        };
        let caret = ui.px(18.0);
        // Before a placeholder the caret sits just left of it, not on it.
        let caret_x = if query.is_empty() {
            tx.saturating_sub(ui.px(3.0))
        } else {
            tx + drawn.min(room) + 1
        };
        fill_rect(
            frame,
            caret_x,
            mid.saturating_sub(caret / 2),
            ui.px(1.5).max(1),
            caret,
            pal.accent,
        );
    }

    let items = launcher.items();
    if items.is_empty() {
        return;
    }
    let line = rect.y + ui.px(INPUT);
    fill_rect(frame, rect.x, line, rect.w, 1, pal.border);
    let row_h = ui.px(ROW);
    let inset = rect.x + ui.px(PAD);
    let width = rect.w.saturating_sub(2 * ui.px(PAD));
    for (i, item) in items.iter().enumerate() {
        let top = line + 1 + ui.px(PAD) + i as u32 * row_h;
        let mid = top + row_h / 2;
        let selected = i == launcher.selected();
        if selected {
            ui::round_rect(frame, inset, top, width, row_h, ui.px(6.0), pal.highlight);
        }
        let cx = rect.x + ui.px(22.0);
        match item {
            Item::App(app)
                if let Some(Some(image)) = content.icons.and_then(|icons| icons.get(&app.id)) =>
            {
                let size = ui.px(ICON);
                draw_icon(
                    frame,
                    image,
                    cx.saturating_sub(size / 2) as i32,
                    mid.saturating_sub(size / 2) as i32,
                    size,
                );
            }
            _ => {
                let (glyph, color) = glyph(item, pal);
                ui.icon(frame, cx, mid, glyph, 13.0, color);
            }
        }
        let right = (inset + width).saturating_sub(ui.px(12.0));
        let side = if selected {
            hint(item, content.in_window)
        } else {
            detail(item)
        };
        let side_x = if side.is_empty() {
            right
        } else {
            ui.text_right(frame, right, mid, side, small, pal.label)
        };
        let room = side_x.saturating_sub(tx + ui.px(12.0));
        ui.text_mid(frame, tx, mid, item.text(), body, pal.fg, room);
    }
}

fn glyph(item: &Item, pal: &Palette) -> (char, u32) {
    match item {
        Item::Calc { .. } => (icon::CALCULATOR, pal.yellow),
        Item::Session(_) => (icon::TERMINAL, pal.fg),
        Item::Task(_) => (icon::PLAY, pal.green),
        Item::NewSession(profile) => match profile.kind {
            Kind::Default | Kind::Shell | Kind::Wsl => (icon::TERMINAL, pal.accent),
            Kind::Ssh => (icon::REMOTE, pal.cyan),
            Kind::Admin => (icon::ADMIN, pal.yellow),
        },
        Item::App(_) => (icon::APPS, pal.blue),
        Item::Builtin(_) => (icon::COLOR, pal.magenta),
        Item::Run(_) => (icon::CHEVRON_RIGHT, pal.green),
        Item::History(_) => (icon::HISTORY, pal.label),
        Item::Search { .. } => (icon::GLOBE, pal.cyan),
    }
}

/// What a row says at its end: where it is from.
fn detail(item: &Item) -> &str {
    match item {
        Item::Calc { .. } => "Math",
        Item::Session(entry) | Item::Task(entry) => &entry.detail,
        Item::NewSession(profile) => match profile.kind {
            Kind::Shell => "New session",
            Kind::Admin => "New window",
            Kind::Default => "Default shell",
            kind => kind.word(),
        },
        Item::App(_) => "App",
        Item::Builtin(_) => "Windows",
        Item::Run(_) => "Command",
        Item::History(_) => "Recent",
        Item::Search { .. } => "Web",
    }
}

/// What Enter does, on the selected row.
fn hint(item: &Item, in_window: bool) -> &'static str {
    match item {
        Item::Calc { .. } => "Enter copies",
        Item::Session(_) => "Enter switches",
        Item::Task(_) => "Enter runs",
        Item::NewSession(_) | Item::App(_) => "Enter opens",
        Item::Builtin(_) => "Enter applies",
        Item::Search { .. } => "Enter searches",
        Item::Run(_) | Item::History(_) if in_window => {
            "Enter runs here \u{b7} Ctrl+Enter in a new session"
        }
        Item::Run(_) | Item::History(_) => "Enter runs \u{b7} Ctrl+Enter in a terminal",
    }
}

/// Draws a premultiplied icon into a `size` square, scaling it by nearest
/// pixel when the shell returned another size.
fn draw_icon(frame: &mut Frame<'_>, icon: &Icon, x: i32, y: i32, size: u32) {
    for dy in 0..size {
        let py = y + dy as i32;
        if py < 0 || py >= frame.height as i32 {
            continue;
        }
        let sy = (dy * icon.height / size) as usize;
        for dx in 0..size {
            let px = x + dx as i32;
            if px < 0 || px >= frame.width as i32 {
                continue;
            }
            let sx = (dx * icon.width / size) as usize;
            let src = icon.pixels[sy * icon.width as usize + sx];
            let alpha = src >> 24;
            if alpha == 0 {
                continue;
            }
            let i = (py as u32 * frame.width + px as u32) as usize;
            let dst = frame.pixels[i];
            let channel = |shift: u32| {
                let s = (src >> shift) & 0xff;
                let d = (dst >> shift) & 0xff;
                (s + d * (255 - alpha) / 255).min(255) << shift
            };
            frame.pixels[i] = channel(16) | channel(8) | channel(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_found_below_the_query() {
        let ui = Ui::new(1.0);
        let rect = Rect {
            x: 100,
            y: 50,
            w: 576,
            h: height(&ui, 3),
        };
        assert_eq!(rect.h, 52 + 12 + 120 + 1);
        let first = f64::from(50 + 52 + 1 + 6);
        assert_eq!(row_at(&ui, rect, 3, 200.0, first + 1.0), Some(0));
        assert_eq!(row_at(&ui, rect, 3, 200.0, first + 85.0), Some(2));
        assert_eq!(row_at(&ui, rect, 3, 200.0, 60.0), None, "the query");
        assert_eq!(row_at(&ui, rect, 3, 50.0, first), None, "outside");
    }
}
