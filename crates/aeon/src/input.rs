//! What keys, pastes, and the mouse send to the shell. Pure, so every
//! encoding is tested without a window.

use winit::keyboard::{Key, ModifiersState, NamedKey};

/// The bytes a key sends, or `None` for keys that send nothing. `text` is
/// the key's text with all modifiers applied (Ctrl+C is `"\x03"`).
pub fn key_bytes(
    key: &Key,
    text: Option<&str>,
    mods: ModifiersState,
    app_cursor: bool,
) -> Option<Vec<u8>> {
    let csi = |s: &str| format!("\x1b[{s}").into_bytes();
    // xterm's modifier parameter: 1 + Shift 1 + Alt 2 + Ctrl 4.
    let modifier = 1
        + u8::from(mods.shift_key())
        + 2 * u8::from(mods.alt_key())
        + 4 * u8::from(mods.control_key());
    let cursor = |letter: char| {
        if modifier > 1 {
            csi(&format!("1;{modifier}{letter}"))
        } else if app_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            csi(&letter.to_string())
        }
    };
    let tilde = |n: u8| {
        if modifier > 1 {
            csi(&format!("{n};{modifier}~"))
        } else {
            csi(&format!("{n}~"))
        }
    };
    let bytes = match key {
        Key::Named(named) => match named {
            NamedKey::Enter => b"\r".to_vec(),
            NamedKey::Backspace if mods.control_key() => b"\x08".to_vec(),
            NamedKey::Backspace => b"\x7f".to_vec(),
            NamedKey::Tab if mods.shift_key() => csi("Z"),
            NamedKey::Tab => b"\t".to_vec(),
            NamedKey::Escape => b"\x1b".to_vec(),
            NamedKey::ArrowUp => cursor('A'),
            NamedKey::ArrowDown => cursor('B'),
            NamedKey::ArrowRight => cursor('C'),
            NamedKey::ArrowLeft => cursor('D'),
            NamedKey::Home => cursor('H'),
            NamedKey::End => cursor('F'),
            NamedKey::Insert => tilde(2),
            NamedKey::Delete => tilde(3),
            NamedKey::PageUp => tilde(5),
            NamedKey::PageDown => tilde(6),
            NamedKey::F1 => function(1, modifier),
            NamedKey::F2 => function(2, modifier),
            NamedKey::F3 => function(3, modifier),
            NamedKey::F4 => function(4, modifier),
            NamedKey::F5 => tilde(15),
            NamedKey::F6 => tilde(17),
            NamedKey::F7 => tilde(18),
            NamedKey::F8 => tilde(19),
            NamedKey::F9 => tilde(20),
            NamedKey::F10 => tilde(21),
            NamedKey::F11 => tilde(23),
            NamedKey::F12 => tilde(24),
            NamedKey::Space => with_alt(if mods.control_key() { "\0" } else { " " }, mods),
            _ => return text.map(|t| with_alt(t, mods)),
        },
        _ => return text.filter(|t| !t.is_empty()).map(|t| with_alt(t, mods)),
    };
    Some(bytes)
}

/// F1 to F4 are `ESC O P` to `ESC O S`, or CSI with a modifier.
fn function(n: u8, modifier: u8) -> Vec<u8> {
    let letter = char::from(b'O' + n);
    if modifier > 1 {
        format!("\x1b[1;{modifier}{letter}").into_bytes()
    } else {
        format!("\x1bO{letter}").into_bytes()
    }
}

/// Alt sends ESC before the key's text. Ctrl+Alt is AltGr on Windows
/// keyboards, which types characters such as `@`, so it gets no prefix.
fn with_alt(text: &str, mods: ModifiersState) -> Vec<u8> {
    if mods.alt_key() && !mods.control_key() {
        let mut bytes = vec![0x1b];
        bytes.extend_from_slice(text.as_bytes());
        bytes
    } else {
        text.as_bytes().to_vec()
    }
}

/// A paste: newlines become carriage returns, as a typed Enter, and a
/// program that asked for bracketed paste gets the markers around it.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    let mut bytes = Vec::with_capacity(text.len() + 12);
    if bracketed {
        // A pasted end marker would end the paste early; drop it.
        let text = text.replace("\x1b[201~", "");
        bytes.extend_from_slice(b"\x1b[200~");
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
    } else {
        bytes.extend_from_slice(text.as_bytes());
    }
    bytes
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    /// Motion with no button held.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

/// A mouse report for a program that enabled mouse tracking. `col` and
/// `row` are zero-based cells.
pub fn mouse_report(
    button: Button,
    action: MouseAction,
    mods: ModifiersState,
    col: usize,
    row: usize,
    sgr: bool,
) -> Vec<u8> {
    let mut code: u32 = match button {
        Button::Left => 0,
        Button::Middle => 1,
        Button::Right => 2,
        Button::None => 3,
        Button::WheelUp => 64,
        Button::WheelDown => 65,
    };
    if action == MouseAction::Motion {
        code += 32;
    }
    code += 4 * u32::from(mods.shift_key())
        + 8 * u32::from(mods.alt_key())
        + 16 * u32::from(mods.control_key());
    let (x, y) = (col + 1, row + 1);
    if sgr {
        let end = if action == MouseAction::Release {
            'm'
        } else {
            'M'
        };
        return format!("\x1b[<{code};{x};{y}{end}").into_bytes();
    }
    // The original encoding has no release button and tops out at 223.
    if action == MouseAction::Release {
        code = (code & !3) | 3;
    }
    let byte = |v: usize| (32 + v.min(223)) as u8;
    vec![
        0x1b,
        b'[',
        b'M',
        (32 + code.min(223)) as u8,
        byte(x),
        byte(y),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(k: NamedKey) -> Key {
        Key::Named(k)
    }

    #[test]
    fn arrows_follow_cursor_key_mode_and_modifiers() {
        let none = ModifiersState::empty();
        assert_eq!(
            key_bytes(&named(NamedKey::ArrowUp), None, none, false).unwrap(),
            b"\x1b[A"
        );
        assert_eq!(
            key_bytes(&named(NamedKey::ArrowUp), None, none, true).unwrap(),
            b"\x1bOA"
        );
        assert_eq!(
            key_bytes(
                &named(NamedKey::ArrowLeft),
                None,
                ModifiersState::CONTROL,
                true
            )
            .unwrap(),
            b"\x1b[1;5D"
        );
        assert_eq!(
            key_bytes(&named(NamedKey::PageUp), None, none, false).unwrap(),
            b"\x1b[5~"
        );
        assert_eq!(
            key_bytes(&named(NamedKey::F1), None, none, false).unwrap(),
            b"\x1bOP"
        );
        assert_eq!(
            key_bytes(&named(NamedKey::F5), None, ModifiersState::SHIFT, false).unwrap(),
            b"\x1b[15;2~"
        );
        assert_eq!(
            key_bytes(&named(NamedKey::Tab), None, ModifiersState::SHIFT, false).unwrap(),
            b"\x1b[Z"
        );
    }

    #[test]
    fn alt_prefixes_escape_but_altgr_does_not() {
        let key = Key::Character("x".into());
        assert_eq!(
            key_bytes(&key, Some("x"), ModifiersState::ALT, false).unwrap(),
            b"\x1bx"
        );
        let altgr = ModifiersState::ALT | ModifiersState::CONTROL;
        assert_eq!(
            key_bytes(&Key::Character("@".into()), Some("@"), altgr, false).unwrap(),
            b"@"
        );
        assert_eq!(
            key_bytes(&key, Some("\x03"), ModifiersState::CONTROL, false).unwrap(),
            b"\x03"
        );
    }

    #[test]
    fn pastes_send_returns_and_brackets() {
        assert_eq!(paste_bytes("a\r\nb\nc", false), b"a\rb\rc");
        assert_eq!(paste_bytes("ls", true), b"\x1b[200~ls\x1b[201~");
        assert_eq!(paste_bytes("x\x1b[201~y", true), b"\x1b[200~xy\x1b[201~");
    }

    #[test]
    fn mouse_reports_in_both_encodings() {
        let none = ModifiersState::empty();
        assert_eq!(
            mouse_report(Button::Left, MouseAction::Press, none, 0, 0, true),
            b"\x1b[<0;1;1M"
        );
        assert_eq!(
            mouse_report(Button::Left, MouseAction::Release, none, 4, 2, true),
            b"\x1b[<0;5;3m"
        );
        assert_eq!(
            mouse_report(
                Button::WheelUp,
                MouseAction::Press,
                ModifiersState::CONTROL,
                1,
                1,
                true
            ),
            b"\x1b[<80;2;2M"
        );
        assert_eq!(
            mouse_report(Button::Left, MouseAction::Motion, none, 1, 1, true),
            b"\x1b[<32;2;2M"
        );
        assert_eq!(
            mouse_report(Button::Left, MouseAction::Press, none, 0, 0, false),
            [0x1b, b'[', b'M', 32, 33, 33]
        );
        assert_eq!(
            mouse_report(Button::Left, MouseAction::Release, none, 0, 0, false)[3],
            35
        );
    }
}
