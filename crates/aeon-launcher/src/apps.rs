//! Installed apps, as the Start menu lists them: desktop programs and Store
//! apps alike, each with the AppUserModelID that opens it.

use std::collections::HashMap;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct App {
    pub name: String,
    /// What `shell:AppsFolder\<id>` opens.
    pub id: String,
    /// Times it was opened from the launcher; ties in a search go to the
    /// app you actually use.
    pub uses: u32,
}

/// Fills in [`App::uses`] from the launcher's record of opened apps.
pub fn count_uses(apps: &mut [App], uses: &HashMap<String, u32>) {
    for app in apps {
        app.uses = uses.get(&app.id).copied().unwrap_or(0);
    }
}

/// How many apps the launcher lists for one query, so commands and history
/// still fit below them.
pub(crate) const MAX_APPS: usize = 4;

/// Asks Windows for the Start menu's app list (`Get-StartApps`). Starting
/// Windows PowerShell makes this take about a second: never call it on the UI
/// thread. Empty when PowerShell is unavailable.
pub fn load() -> Vec<App> {
    let mut command = Command::new("powershell.exe");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            // UTF-8 out, or app names outside the console code page arrive garbled.
            "[Console]::OutputEncoding = [Text.Encoding]::UTF8; \
             Get-StartApps | ForEach-Object { $_.Name + [char]9 + $_.AppID }",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .output()
        .map(|out| parse(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
}

/// One `name<TAB>id` per line, sorted and without duplicates.
pub(crate) fn parse(text: &str) -> Vec<App> {
    let mut apps: Vec<App> = text
        .lines()
        .filter_map(|line| line.trim_start_matches('\u{feff}').split_once('\t'))
        .map(|(name, id)| (name.trim(), id.trim()))
        .filter(|(name, id)| !name.is_empty() && !id.is_empty())
        .map(|(name, id)| App {
            name: name.to_owned(),
            id: id.to_owned(),
            uses: 0,
        })
        .collect();
    apps.sort();
    apps.dedup();
    apps
}

/// Opens an app through the shell, which knows how to start both desktop
/// programs and Store apps from their id.
pub fn open(id: &str) -> std::io::Result<()> {
    Command::new("explorer.exe")
        .arg(format!("shell:AppsFolder\\{id}"))
        .spawn()
        .map(drop)
}

/// Apps whose name matches `query`, best first: the whole name, then its
/// start, then the start of any word, then its initials (`vsc` for Visual
/// Studio Code), then anywhere. A single letter only matches word starts:
/// `d` should find Discord, not every name with a d in it.
pub(crate) fn search<'a>(apps: &'a [App], query: &str) -> Vec<&'a App> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let terms: Vec<&str> = query.split_whitespace().collect();
    let mut scored: Vec<(u8, &App)> = apps
        .iter()
        .filter_map(|app| score(&app.name.to_lowercase(), &query, &terms).map(|s| (s, app)))
        .collect();
    scored.sort_by_key(|(score, app)| (*score, std::cmp::Reverse(app.uses), app.name.len()));
    scored
        .into_iter()
        .map(|(_, app)| app)
        .take(MAX_APPS)
        .collect()
}

fn score(name: &str, query: &str, terms: &[&str]) -> Option<u8> {
    let words: Vec<&str> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let initials: String = words.iter().filter_map(|w| w.chars().next()).collect();
    if name == query {
        Some(0)
    } else if name.starts_with(query) {
        Some(1)
    } else if words.iter().any(|w| w.starts_with(query)) {
        Some(2)
    } else if terms.len() == 1 && query.chars().count() >= 2 && initials.starts_with(query) {
        Some(3)
    } else if query.chars().count() >= 2 && terms.iter().all(|t| name.contains(t)) {
        Some(4)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apps(names: &[&str]) -> Vec<App> {
        names
            .iter()
            .map(|n| App {
                name: n.to_string(),
                id: format!("id.{n}"),
                uses: 0,
            })
            .collect()
    }

    #[test]
    fn apps_you_open_win_ties() {
        let mut all = apps(&["T3 Code", "Visual Studio Code"]);
        assert_eq!(
            names(&search(&all, "code")),
            ["T3 Code", "Visual Studio Code"]
        );
        count_uses(
            &mut all,
            &HashMap::from([("id.Visual Studio Code".to_owned(), 2)]),
        );
        assert_eq!(
            names(&search(&all, "code")),
            ["Visual Studio Code", "T3 Code"]
        );
    }

    fn names<'a>(found: &[&'a App]) -> Vec<&'a str> {
        found.iter().map(|a| a.name.as_str()).collect()
    }

    #[test]
    fn parses_get_start_apps_output() {
        let parsed = parse(
            "\u{feff}Visual Studio Code\tMicrosoft.VisualStudioCode\r\n\
             Discord\tcom.squirrel.Discord.Discord\r\n\
             not an app line\r\n\
             Discord\tcom.squirrel.Discord.Discord\r\n",
        );
        assert_eq!(
            parsed,
            [
                App {
                    name: "Discord".into(),
                    id: "com.squirrel.Discord.Discord".into(),
                    uses: 0,
                },
                App {
                    name: "Visual Studio Code".into(),
                    id: "Microsoft.VisualStudioCode".into(),
                    uses: 0,
                },
            ]
        );
    }

    #[test]
    fn ranks_name_start_word_start_initials_then_anywhere() {
        let all = apps(&[
            "Discord",
            "Disk Cleanup",
            "Visual Studio Code",
            "Code Writer",
            "Unicode Map",
        ]);
        assert_eq!(names(&search(&all, "disc")), ["Discord"]);
        assert_eq!(
            names(&search(&all, "code")),
            ["Code Writer", "Visual Studio Code", "Unicode Map"]
        );
        assert_eq!(names(&search(&all, "vsc")), ["Visual Studio Code"]);
        assert_eq!(names(&search(&all, "studio vis")), ["Visual Studio Code"]);
    }

    #[test]
    fn single_letters_match_word_starts_only() {
        let all = apps(&["Discord", "Notepad", "Windows Defender"]);
        assert_eq!(names(&search(&all, "d")), ["Discord", "Windows Defender"]);
        assert!(search(&all, "").is_empty());
    }

    #[test]
    fn caps_the_list() {
        let all = apps(&["a1", "a2", "a3", "a4", "a5", "a6"]);
        assert_eq!(search(&all, "a").len(), MAX_APPS);
    }

    #[cfg(windows)]
    #[test]
    fn lists_installed_apps() {
        // Every Windows install has at least a few Start menu entries; an
        // empty list means the PowerShell call or the parsing broke.
        let apps = load();
        assert!(!apps.is_empty(), "Get-StartApps returned nothing");
        assert!(apps.iter().all(|a| !a.name.is_empty() && !a.id.is_empty()));
    }
}
