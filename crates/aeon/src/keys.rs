//! Key combinations as written in config.toml (`"Ctrl+Shift+T"`), matched
//! against winit's key events.

use std::fmt;
use std::str::FromStr;

use winit::keyboard::{Key, ModifiersState, NamedKey};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combo {
    mods: ModifiersState,
    key: ComboKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ComboKey {
    /// A printable key, lowercase.
    Char(String),
    Named(NamedKey),
}

/// Named keys by their config spelling.
const NAMED: &[(&str, NamedKey)] = &[
    ("Tab", NamedKey::Tab),
    ("Enter", NamedKey::Enter),
    ("Space", NamedKey::Space),
    ("Escape", NamedKey::Escape),
    ("Backspace", NamedKey::Backspace),
    ("Delete", NamedKey::Delete),
    ("Insert", NamedKey::Insert),
    ("Home", NamedKey::Home),
    ("End", NamedKey::End),
    ("PageUp", NamedKey::PageUp),
    ("PageDown", NamedKey::PageDown),
    ("Up", NamedKey::ArrowUp),
    ("Down", NamedKey::ArrowDown),
    ("Left", NamedKey::ArrowLeft),
    ("Right", NamedKey::ArrowRight),
    ("F1", NamedKey::F1),
    ("F2", NamedKey::F2),
    ("F3", NamedKey::F3),
    ("F4", NamedKey::F4),
    ("F5", NamedKey::F5),
    ("F6", NamedKey::F6),
    ("F7", NamedKey::F7),
    ("F8", NamedKey::F8),
    ("F9", NamedKey::F9),
    ("F10", NamedKey::F10),
    ("F11", NamedKey::F11),
    ("F12", NamedKey::F12),
];

impl Combo {
    /// Builds a combo from a key press, or `None` for a modifier alone.
    pub fn from_event(key: &Key, mods: ModifiersState) -> Option<Self> {
        let key = match key {
            Key::Named(named) => ComboKey::Named(NAMED.iter().find(|(_, n)| n == named)?.1),
            Key::Character(c) if !c.trim().is_empty() => ComboKey::Char(c.to_lowercase()),
            _ => return None,
        };
        Some(Self { mods, key })
    }

    pub fn matches(&self, key: &Key, mods: ModifiersState) -> bool {
        Self::from_event(key, mods).is_some_and(|pressed| pressed == *self)
    }

    pub fn has_modifier(&self) -> bool {
        !self.mods.is_empty()
    }
}

impl FromStr for Combo {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // "Ctrl++" names the plus key: the last part is the key, whatever it is.
        let (mods_part, key_part) = match s.rsplit_once('+') {
            Some((mods, "")) if mods.ends_with('+') => (&mods[..mods.len() - 1], "+"),
            Some((mods, key)) => (mods, key),
            None => ("", s),
        };
        let mut mods = ModifiersState::empty();
        for part in mods_part
            .split('+')
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            mods |= match part.to_lowercase().as_str() {
                "ctrl" | "control" => ModifiersState::CONTROL,
                "shift" => ModifiersState::SHIFT,
                "alt" => ModifiersState::ALT,
                "win" | "super" | "meta" => ModifiersState::SUPER,
                _ => return Err(format!("unknown modifier {part:?} in {s:?}")),
            };
        }
        let key_part = key_part.trim();
        let key = if let Some((_, named)) =
            NAMED.iter().find(|(n, _)| n.eq_ignore_ascii_case(key_part))
        {
            ComboKey::Named(*named)
        } else if key_part.chars().count() == 1 {
            ComboKey::Char(key_part.to_lowercase())
        } else {
            return Err(format!("unknown key {key_part:?} in {s:?}"));
        };
        Ok(Self { mods, key })
    }
}

impl fmt::Display for Combo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (flag, name) in [
            (ModifiersState::CONTROL, "Ctrl+"),
            (ModifiersState::ALT, "Alt+"),
            (ModifiersState::SHIFT, "Shift+"),
            (ModifiersState::SUPER, "Win+"),
        ] {
            if self.mods.contains(flag) {
                f.write_str(name)?;
            }
        }
        match &self.key {
            ComboKey::Char(c) => f.write_str(&c.to_uppercase()),
            ComboKey::Named(named) => {
                let name = NAMED
                    .iter()
                    .find(|(_, n)| n == named)
                    .map_or("?", |(n, _)| n);
                f.write_str(name)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use winit::keyboard::SmolStr;

    use super::*;

    fn ch(c: &str) -> Key {
        Key::Character(SmolStr::new(c))
    }

    #[test]
    fn parses_matches_and_prints() {
        let combo: Combo = "Ctrl+Shift+T".parse().unwrap();
        let mods = ModifiersState::CONTROL | ModifiersState::SHIFT;
        // With Shift held, winit reports the uppercase character.
        assert!(combo.matches(&ch("T"), mods));
        assert!(!combo.matches(&ch("T"), ModifiersState::CONTROL));
        assert_eq!(combo.to_string(), "Ctrl+Shift+T");
    }

    #[test]
    fn named_and_punctuation_keys() {
        let tab: Combo = "ctrl+tab".parse().unwrap();
        assert!(tab.matches(&Key::Named(NamedKey::Tab), ModifiersState::CONTROL));
        assert_eq!(tab.to_string(), "Ctrl+Tab");
        let comma: Combo = "Ctrl+,".parse().unwrap();
        assert!(comma.matches(&ch(","), ModifiersState::CONTROL));
        let plus: Combo = "Ctrl++".parse().unwrap();
        assert!(plus.matches(&ch("+"), ModifiersState::CONTROL));
    }

    #[test]
    fn rejects_unknown_parts() {
        assert!("Hyper+T".parse::<Combo>().is_err());
        assert!("Ctrl+Banana".parse::<Combo>().is_err());
    }

    #[test]
    fn events_round_trip_through_text() {
        let mods = ModifiersState::CONTROL | ModifiersState::ALT;
        let combo = Combo::from_event(&ch("k"), mods).unwrap();
        assert_eq!(combo.to_string().parse::<Combo>().unwrap(), combo);
        assert!(Combo::from_event(&Key::Named(NamedKey::Control), mods).is_none());
    }
}
