//! Changes single keys of `config.toml` the way a person would: everything
//! else in the file, comments and layout included, stays exactly as it was.

use std::path::Path;

use toml_edit::{Array, DocumentMut, Item, Table, value};

/// A value the settings panel writes.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<String>),
}

/// One key to set, or to remove when `value` is `None` (back to the default).
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub table: &'static str,
    pub key: &'static str,
    pub value: Option<Value>,
}

/// Applies `changes` to the text of a config file.
pub fn apply(text: &str, changes: &[Change]) -> Result<String, String> {
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
    for change in changes {
        let table = doc
            .entry(change.table)
            .or_insert_with(|| Item::Table(Table::new()))
            .as_table_mut()
            .ok_or_else(|| format!("`{}` is not a table", change.table))?;
        match &change.value {
            None => {
                table.remove(change.key);
            }
            Some(v) => {
                let item = match v {
                    Value::Bool(b) => value(*b),
                    Value::Int(i) => value(*i),
                    Value::Float(f) => value(*f),
                    Value::Str(s) => value(s.as_str()),
                    Value::List(items) => value(items.iter().collect::<Array>()),
                };
                // Keep a comment written after the old value on its line.
                let decor = table
                    .get(change.key)
                    .and_then(Item::as_value)
                    .map(|old| old.decor().clone());
                table[change.key] = item;
                if let (Some(decor), Some(new)) = (decor, table[change.key].as_value_mut()) {
                    *new.decor_mut() = decor;
                }
            }
        }
    }
    Ok(doc.to_string())
}

/// Applies `changes` to the file at `path`, creating it when missing. The
/// new text is written next to it first, so a crash never leaves half a file.
pub fn write(path: &Path, changes: &[Change]) -> Result<(), String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("could not read {}: {e}", path.display())),
    };
    let new = apply(&text, changes)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let temp = path.with_extension("toml.tmp");
    std::fs::write(&temp, new).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    fn set(table: &'static str, key: &'static str, value: Option<Value>) -> Change {
        Change { table, key, value }
    }

    #[test]
    fn keeps_comments_and_other_keys() {
        let text = "# my settings\n[font]\n# I like it big\nsize = 14.0 # pt\npath = \"x.ttf\"\n\n[window]\npadding = 4\n";
        let out = apply(text, &[set("font", "size", Some(Value::Float(16.0)))]).unwrap();
        assert_eq!(
            out,
            "# my settings\n[font]\n# I like it big\nsize = 16.0 # pt\npath = \"x.ttf\"\n\n[window]\npadding = 4\n"
        );
    }

    #[test]
    fn creates_missing_tables_and_files() {
        let out = apply(
            "",
            &[set(
                "launcher",
                "hotkey",
                Some(Value::Str("Ctrl+Alt+K".into())),
            )],
        )
        .unwrap();
        let config = Config::parse(&out).unwrap();
        assert_eq!(config.launcher.hotkey, "Ctrl+Alt+K");
    }

    #[test]
    fn removing_a_key_restores_its_default() {
        let out = apply(
            "[shell]\nprogram = \"cmd.exe\"\nargs = [\"/k\"]\n",
            &[set("shell", "program", None), set("shell", "args", None)],
        )
        .unwrap();
        assert_eq!(Config::parse(&out).unwrap().shell, Config::default().shell);
    }

    #[test]
    fn writes_lists_and_bools() {
        let out = apply(
            "",
            &[
                set(
                    "shell",
                    "args",
                    Some(Value::List(vec!["--login".into(), "-i".into()])),
                ),
                set("bar", "enabled", Some(Value::Bool(false))),
            ],
        )
        .unwrap();
        let config = Config::parse(&out).unwrap();
        assert_eq!(config.shell.args, ["--login", "-i"]);
        assert!(!config.bar.enabled);
    }

    #[test]
    fn rejects_broken_files_instead_of_overwriting_them() {
        assert!(apply("[font\nsize = ", &[set("font", "size", None)]).is_err());
    }

    #[test]
    fn write_round_trips_through_a_file() {
        let dir = std::env::temp_dir().join(format!("aeon-edit-{}", std::process::id()));
        let path = dir.join("config.toml");
        write(&path, &[set("window", "padding", Some(Value::Int(12)))]).unwrap();
        write(&path, &[set("font", "size", Some(Value::Float(13.0)))]).unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!((config.window.padding, config.font.size), (12, 13.0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
