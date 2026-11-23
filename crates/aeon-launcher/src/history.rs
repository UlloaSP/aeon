//! Launcher history: what was run from the launcher first, then the shell
//! history Aeon's PowerShell keeps, newest first, without duplicates.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Plenty to search, small enough to load on every open.
const LIMIT: usize = 2000;

/// `%APPDATA%\aeon\history`, shared with the shell integration.
pub fn dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("aeon").join("history"))
}

/// Reads the launcher history and the shell histories in `dir`.
pub fn load(dir: &Path) -> Vec<String> {
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap_or_default();
    merge([
        lines(&read("launcher.txt")),
        shell_lines(&read("pwsh.txt")),
        shell_lines(&read("powershell.txt")),
    ])
}

/// Records a command run from the launcher.
pub fn append(dir: &Path, command: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("launcher.txt"))?;
    writeln!(file, "{}", command.replace(['\r', '\n'], " "))
}

/// Records an app opened from the launcher, by its id.
pub fn record_app(dir: &Path, id: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("apps.txt"))?;
    writeln!(file, "{id}")
}

/// How many times each app id was opened from the launcher.
pub fn app_uses(dir: &Path) -> HashMap<String, u32> {
    let mut uses = HashMap::new();
    let text = std::fs::read_to_string(dir.join("apps.txt")).unwrap_or_default();
    for id in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        *uses.entry(id.to_owned()).or_insert(0) += 1;
    }
    uses
}

/// Concatenates sources (each already newest first), keeping the first copy.
fn merge(sources: [Vec<String>; 3]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    sources
        .into_iter()
        .flatten()
        .filter(|line| seen.insert(line.clone()))
        .take(LIMIT)
        .collect()
}

fn lines(text: &str) -> Vec<String> {
    text.lines()
        .rev()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// PSReadLine history, minus multi-line commands: their lines end in a
/// backtick, and a fragment of one is not something to run again.
fn shell_lines(text: &str) -> Vec<String> {
    let mut single = Vec::new();
    let mut continued = false;
    for line in text.lines() {
        let is_continuation = continued;
        continued = line.ends_with('`');
        if !is_continuation && !continued {
            single.push(line);
        }
    }
    lines(&single.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_first_then_shell_newest_first_deduplicated() {
        let merged = merge([
            lines("code .\ncalc\n"),
            shell_lines("git status\ncode .\ncargo test\n"),
            Vec::new(),
        ]);
        assert_eq!(merged, ["calc", "code .", "cargo test", "git status"]);
    }

    #[test]
    fn multi_line_shell_commands_are_skipped() {
        let history = shell_lines("ls\nforeach ($x in 1..3) {`\n  $x`\n}\ngit log\n");
        assert_eq!(history, ["git log", "ls"]);
    }

    #[test]
    fn app_uses_count_each_open() {
        let dir = std::env::temp_dir().join(format!("aeon-app-uses-{}", std::process::id()));
        record_app(&dir, "Microsoft.VisualStudioCode").unwrap();
        record_app(&dir, "Microsoft.VisualStudioCode").unwrap();
        record_app(&dir, "com.squirrel.Discord.Discord").unwrap();
        let uses = app_uses(&dir);
        assert_eq!(uses["Microsoft.VisualStudioCode"], 2);
        assert_eq!(uses["com.squirrel.Discord.Discord"], 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("aeon-history-{}", std::process::id()));
        append(&dir, "code C:\\repo").unwrap();
        append(&dir, "multi\nline").unwrap();
        assert_eq!(load(&dir), ["multi line", "code C:\\repo"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
