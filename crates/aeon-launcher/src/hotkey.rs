//! Hotkeys as written in config.toml: `"Ctrl+Alt+Space"`.

use std::fmt;
use std::str::FromStr;

/// Win32 `MOD_*` flags and a virtual-key code, as `RegisterHotKey` takes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub modifiers: u32,
    pub key: u32,
}

pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;

impl Default for Hotkey {
    fn default() -> Self {
        Self {
            modifiers: MOD_CONTROL | MOD_ALT,
            key: 0x20,
        }
    }
}

impl FromStr for Hotkey {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut modifiers = 0;
        let mut key = None;
        for part in s.split('+').map(str::trim) {
            let lower = part.to_lowercase();
            let modifier = match lower.as_str() {
                "ctrl" | "control" => MOD_CONTROL,
                "alt" => MOD_ALT,
                "shift" => MOD_SHIFT,
                "win" | "super" | "meta" => MOD_WIN,
                _ => 0,
            };
            if modifier != 0 {
                modifiers |= modifier;
                continue;
            }
            if key.is_some() {
                return Err(format!("{s:?} has more than one key"));
            }
            key =
                Some(virtual_key(&lower).ok_or_else(|| format!("unknown key {part:?} in {s:?}"))?);
        }
        let key = key.ok_or_else(|| format!("{s:?} has no key"))?;
        if modifiers == 0 {
            return Err(format!("{s:?} needs at least one of Ctrl, Alt, Shift, Win"));
        }
        Ok(Self { modifiers, key })
    }
}

fn virtual_key(name: &str) -> Option<u32> {
    let named = match name {
        "space" => 0x20,
        "enter" | "return" => 0x0D,
        "tab" => 0x09,
        "`" | "backquote" => 0xC0,
        _ => 0,
    };
    if named != 0 {
        return Some(named);
    }
    if let [c] = name.as_bytes()
        && (c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return Some(u32::from(c.to_ascii_uppercase()));
    }
    let n: u32 = name.strip_prefix('f')?.parse().ok()?;
    (1..=24).contains(&n).then(|| 0x70 + n - 1)
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (flag, name) in [
            (MOD_CONTROL, "Ctrl+"),
            (MOD_ALT, "Alt+"),
            (MOD_SHIFT, "Shift+"),
            (MOD_WIN, "Win+"),
        ] {
            if self.modifiers & flag != 0 {
                f.write_str(name)?;
            }
        }
        match self.key {
            0x20 => f.write_str("Space"),
            0x0D => f.write_str("Enter"),
            0x09 => f.write_str("Tab"),
            0xC0 => f.write_str("`"),
            k @ 0x70..=0x87 => write!(f, "F{}", k - 0x70 + 1),
            k => write!(f, "{}", char::from_u32(k).unwrap_or('?')),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints_round_trip() {
        for text in [
            "Ctrl+Alt+Space",
            "Win+`",
            "Ctrl+Shift+K",
            "Alt+F12",
            "Ctrl+Alt+1",
        ] {
            let hotkey: Hotkey = text.parse().unwrap();
            assert_eq!(hotkey.to_string(), text);
        }
        assert_eq!("ctrl + alt + space".parse(), Ok(Hotkey::default()));
    }

    #[test]
    fn rejects_bad_hotkeys() {
        for text in ["Space", "Ctrl+Alt", "Ctrl+Foo", "Ctrl+A+B", "Ctrl+F25", ""] {
            assert!(text.parse::<Hotkey>().is_err(), "{text:?}");
        }
    }
}
