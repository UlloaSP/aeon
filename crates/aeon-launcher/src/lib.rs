//! The palette: one search box behind the global hotkey and, inside a
//! terminal window, over its sessions too.
//!
//! [`Launcher`] is the box's state and is pure: typing, moving the selection,
//! and choosing produce an [`Action`] that the caller carries out. Win32 lives
//! in `sys`.

pub mod apps;
mod calc;
pub mod history;
mod hotkey;
pub mod icons;
pub mod profiles;
#[cfg(windows)]
pub mod sys;
pub mod update;
pub mod updater;

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};

pub use apps::App;
pub use hotkey::Hotkey;
pub use icons::{Icon, Icons};
pub use profiles::Profile;

/// The drop-down terminal's window class, which the resident finds it by.
/// Development builds use their own, as with the other locks.
pub const DROPDOWN_CLASS: &str = if cfg!(debug_assertions) {
    "AeonDropdownDev"
} else {
    "AeonDropdown"
};

/// How many results the box lists below the query.
pub const MAX_ITEMS: usize = 8;
/// How many profiles the new-session menu lists.
const MAX_MENU: usize = 12;

/// A session or task of the window the palette is over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// What the window knows it by.
    pub id: usize,
    pub title: String,
    /// Shown beside the title, such as the session's project.
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// The query is math; `result` is what Enter copies.
    Calc { result: String },
    /// Go to a session of this window.
    Session(Entry),
    /// Run a task of the active session's project.
    Task(Entry),
    /// Open a new session with a shell, distro, host, or as administrator.
    NewSession(Profile),
    /// One of Aeon's own commands, such as `theme`.
    Builtin(Builtin),
    /// An installed app whose name matches the query.
    App(App),
    /// Run the query as typed.
    Run(String),
    /// A past command that matches the query.
    History(String),
    /// Search the web for the query; `label` is what the list shows.
    Search { query: String, label: String },
}

impl Item {
    pub fn text(&self) -> &str {
        match self {
            Item::Calc { result } => result,
            Item::Session(entry) | Item::Task(entry) => &entry.title,
            Item::NewSession(profile) => &profile.label,
            Item::Builtin(builtin) => builtin.label(),
            Item::App(app) => &app.name,
            Item::Run(command) | Item::History(command) => command,
            Item::Search { label, .. } => label,
        }
    }
}

/// Commands the launcher carries out itself instead of passing to a shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    /// Switch Windows between its light and dark theme.
    Theme(Theme),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Toggle,
    Dark,
    Light,
}

impl Builtin {
    pub fn label(self) -> &'static str {
        match self {
            Builtin::Theme(Theme::Toggle) => "Switch between light and dark theme",
            Builtin::Theme(Theme::Dark) => "Dark theme",
            Builtin::Theme(Theme::Light) => "Light theme",
        }
    }
}

/// `theme`, `theme dark`, `theme light`, and their Spanish names, matched by
/// prefix so `th` already offers them.
fn builtins(query: &str) -> Vec<Builtin> {
    let lower = query.to_lowercase();
    let mut words = lower.split_whitespace();
    let Some(first) = words.next() else {
        return Vec::new();
    };
    let prefix_of = |word: &str, names: &[&str]| names.iter().any(|n| n.starts_with(word));
    if first.chars().count() < 2 || !prefix_of(first, &["theme", "tema"]) {
        return Vec::new();
    }
    match (words.next(), words.next()) {
        (None, _) => vec![
            Builtin::Theme(Theme::Toggle),
            Builtin::Theme(Theme::Dark),
            Builtin::Theme(Theme::Light),
        ],
        (Some(w), None) if prefix_of(w, &["dark", "oscuro"]) => vec![Builtin::Theme(Theme::Dark)],
        (Some(w), None) if prefix_of(w, &["light", "claro"]) => vec![Builtin::Theme(Theme::Light)],
        _ => Vec::new(),
    }
}

/// What the notification-area icon asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    OpenLauncher,
    NewTerminal,
    OpenSettings,
    Quit,
    /// A terminal saved settings; re-read config.toml.
    Reload,
    CheckUpdates,
    DownloadUpdate,
    InstallUpdate,
}

/// What a terminal asks the resident to do, sent to its tray window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidentCommand {
    SettingsChanged = 1,
    CheckUpdates = 2,
    DownloadUpdate = 3,
    InstallUpdate = 4,
}

impl ResidentCommand {
    pub fn from_code(code: usize) -> Option<Self> {
        [
            Self::SettingsChanged,
            Self::CheckUpdates,
            Self::DownloadUpdate,
            Self::InstallUpdate,
        ]
        .into_iter()
        .find(|c| *c as usize == code)
    }

    pub fn tray_command(self) -> TrayCommand {
        match self {
            Self::SettingsChanged => TrayCommand::Reload,
            Self::CheckUpdates => TrayCommand::CheckUpdates,
            Self::DownloadUpdate => TrayCommand::DownloadUpdate,
            Self::InstallUpdate => TrayCommand::InstallUpdate,
        }
    }
}

/// What choosing an item asks the caller to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Start the command in the background and close the launcher.
    Run(String),
    /// Open an Aeon terminal that runs the command.
    RunInTerminal(String),
    /// Put text on the clipboard.
    Copy(String),
    /// Open an installed app by its id.
    OpenApp(String),
    /// Change the Windows theme.
    SetTheme(Theme),
    /// Open a web page in the default browser.
    OpenUrl(String),
    GoToSession(usize),
    RunTask(usize),
    NewSession(Profile),
}

/// What the launcher searches.
#[derive(Debug, Default, Clone)]
pub struct Sources {
    /// Past commands, newest first.
    pub history: Vec<String>,
    pub apps: Vec<App>,
    /// Programs on PATH, lowercase and without extension: what makes a query
    /// look like a command rather than words to search the web for.
    pub commands: HashSet<String>,
    /// Search URL with `{query}` where the words go. Empty: no web search.
    pub search_url: String,
    /// The window's sessions, in sidebar order. None in the global palette.
    pub sessions: Vec<Entry>,
    /// The active project's tasks.
    pub tasks: Vec<Entry>,
    pub profiles: Vec<Profile>,
}

#[derive(Debug, Default)]
pub struct Launcher {
    query: String,
    selected: usize,
    sources: Sources,
    items: Vec<Item>,
    /// Only new-session profiles: the menu behind New session.
    menu: bool,
}

impl Launcher {
    /// Starts an empty query over `sources`.
    pub fn open(sources: Sources) -> Self {
        let mut launcher = Self {
            sources,
            ..Self::default()
        };
        launcher.refresh();
        launcher
    }

    /// The new-session menu: every profile, narrowed by typing.
    pub fn menu(sources: Sources) -> Self {
        let mut launcher = Self {
            sources,
            menu: true,
            ..Self::default()
        };
        launcher.refresh();
        launcher
    }

    pub fn is_menu(&self) -> bool {
        self.menu
    }

    /// Profiles arrived from their background lookup.
    pub fn set_profiles(&mut self, profiles: Vec<Profile>) {
        self.sources.profiles = profiles;
        self.refresh();
    }

    /// The app and command lists arrived while the box is open; keeps the query.
    pub fn set_catalog(&mut self, apps: Vec<App>, commands: HashSet<String>) {
        self.sources.apps = apps;
        self.sources.commands = commands;
        self.refresh();
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn insert(&mut self, text: &str) {
        // Pasted text can hold newlines; the box is one line.
        self.query.extend(text.chars().filter(|c| !c.is_control()));
        self.refresh();
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.refresh();
    }

    /// Ctrl+Backspace: the previous word and the spaces before the caret.
    pub fn delete_word(&mut self) {
        let separators = [' ', '\\', '/'];
        let start = self
            .query
            .trim_end_matches(separators)
            .rfind(separators)
            .map_or(0, |i| i + 1);
        self.query.truncate(start);
        self.refresh();
    }

    pub fn select_next(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + 1) % self.items.len();
        }
    }

    pub fn select_previous(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + self.items.len() - 1) % self.items.len();
        }
    }

    /// Tab: puts the selected command in the box to edit it.
    pub fn complete(&mut self) {
        if let Some(Item::History(command) | Item::Run(command)) = self.items.get(self.selected) {
            self.query = command.clone();
            self.refresh();
        }
    }

    /// Enter (`in_terminal: false`) or Ctrl+Enter (`true`) on the selection.
    pub fn choose(&self, in_terminal: bool) -> Option<Action> {
        Some(match self.items.get(self.selected)? {
            Item::Calc { result } => Action::Copy(result.clone()),
            Item::Run(command) | Item::History(command) if in_terminal => {
                Action::RunInTerminal(command.clone())
            }
            Item::Run(command) | Item::History(command) => Action::Run(command.clone()),
            Item::App(app) => Action::OpenApp(app.id.clone()),
            Item::Builtin(Builtin::Theme(theme)) => Action::SetTheme(*theme),
            Item::Search { query, .. } => {
                Action::OpenUrl(search_url(&self.sources.search_url, query))
            }
            Item::Session(entry) => Action::GoToSession(entry.id),
            Item::Task(entry) => Action::RunTask(entry.id),
            Item::NewSession(profile) => Action::NewSession(profile.clone()),
        })
    }

    /// Selects row `index`, as a click does before choosing it.
    pub fn select(&mut self, index: usize) {
        if index < self.items.len() {
            self.selected = index;
        }
    }

    fn refresh(&mut self) {
        self.items = if self.menu {
            profile_items(&self.query, &self.sources.profiles, MAX_MENU)
        } else {
            items(&self.query, &self.sources)
        };
        self.selected = 0;
    }
}

fn search_item(query: &str) -> Item {
    Item::Search {
        query: query.to_owned(),
        label: format!("Search the web for \u{201c}{query}\u{201d}"),
    }
}

/// `template` with `{query}` replaced by the percent-encoded words, or the
/// words appended when the template has no placeholder.
pub fn search_url(template: &str, query: &str) -> String {
    let mut encoded = String::new();
    for byte in query.trim().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            b' ' => encoded.push('+'),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    if template.contains("{query}") {
        template.replace("{query}", &encoded)
    } else {
        format!("{template}{encoded}")
    }
}

/// Whether the query reads as a command: its first word is a program on
/// PATH, a PowerShell cmdlet (`Get-Process`), a path, or something run before.
fn looks_like_command(query: &str, sources: &Sources) -> bool {
    let Some(first) = query.split_whitespace().next() else {
        return false;
    };
    let lower = first.to_lowercase();
    let stem = lower
        .rsplit_once('.')
        .filter(|(_, ext)| ["exe", "cmd", "bat", "ps1", "com"].contains(ext))
        .map_or(lower.as_str(), |(stem, _)| stem);
    let cmdlet = lower.split_once('-').is_some_and(|(verb, noun)| {
        !verb.is_empty()
            && !noun.is_empty()
            && (verb.chars().chain(noun.chars())).all(|c| c.is_ascii_alphabetic())
    });
    sources.commands.contains(stem)
        || cmdlet
        || first.contains(['\\', '/', ':'])
        || first.starts_with(['.', '$'])
        || sources.history.iter().any(|h| {
            h.split_whitespace()
                .next()
                .is_some_and(|w| w.eq_ignore_ascii_case(first))
        })
}

/// Whether every word of `query` is in `text`, ignoring case.
fn matches_words(query: &str, text: &str) -> bool {
    let text = text.to_lowercase();
    query
        .split_whitespace()
        .all(|word| text.contains(&word.to_lowercase()))
}

/// Profiles whose name or kind has every word of `query`, also reached by
/// "new": "new wsl" lists the distros.
fn profile_items(query: &str, profiles: &[Profile], max: usize) -> Vec<Item> {
    profiles
        .iter()
        .filter(|p| matches_words(query, &format!("new session {} {}", p.label, p.kind.word())))
        .take(max)
        .cloned()
        .map(Item::NewSession)
        .collect()
}

fn items(query: &str, sources: &Sources) -> Vec<Item> {
    let query = query.trim();
    // An empty box over a window lists its sessions, then recent commands.
    if query.is_empty() {
        return sources
            .sessions
            .iter()
            .cloned()
            .map(Item::Session)
            .chain(sources.history.iter().cloned().map(Item::History))
            .take(MAX_ITEMS)
            .collect();
    }
    // `?` forces a web search.
    if let Some(words) = query.strip_prefix('?') {
        let words = words.trim();
        return if words.is_empty() || sources.search_url.is_empty() {
            Vec::new()
        } else {
            vec![search_item(words)]
        };
    }
    let (history, apps) = (&sources.history, &sources.apps);
    let mut items = Vec::new();
    if let Some(value) = calc::evaluate(query) {
        items.push(Item::Calc {
            result: calc::format(value),
        });
    }
    let builtins = builtins(query);
    let is_builtin = !builtins.is_empty();
    items.extend(builtins.into_iter().map(Item::Builtin));
    let entry = |e: &&Entry| matches_words(query, &format!("{} {}", e.title, e.detail));
    items.extend(
        sources
            .sessions
            .iter()
            .filter(entry)
            .cloned()
            .map(Item::Session),
    );
    items.extend(sources.tasks.iter().filter(entry).cloned().map(Item::Task));
    items.extend(profile_items(query, &sources.profiles, 3));
    let found_apps = apps::search(apps, query);
    let app_named_so = found_apps
        .iter()
        .any(|a| a.name.eq_ignore_ascii_case(query));
    items.extend(found_apps.into_iter().cloned().map(Item::App));
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let matches = |command: &&String| {
        let command = command.to_lowercase();
        terms.iter().all(|t| command.contains(t.as_str()))
    };
    // Commands that start like the query rank above ones that merely contain
    // it; within each group, history order (newest first) is kept.
    let lower = query.to_lowercase();
    let (prefix, rest): (Vec<&String>, Vec<&String>) = history
        .iter()
        .filter(matches)
        .partition(|c| c.to_lowercase().starts_with(&lower));
    let found: Vec<&String> = prefix.into_iter().chain(rest).collect();

    // Running the query as a command is offered unless something else
    // already is the query: a result, one of Aeon's commands, a history
    // entry, or an app by that name.
    let exact = found.first().is_some_and(|c| c.as_str() == query);
    let is_math = matches!(items.first(), Some(Item::Calc { .. }));
    let offers = !query.is_empty() && !is_math && !is_builtin;
    let run = (offers && !exact && !app_named_so).then(|| Item::Run(query.to_owned()));
    let search = (offers && !sources.search_url.is_empty()).then(|| search_item(query));
    // Words that are not a command read as a web search: offer that first.
    if looks_like_command(query, sources) {
        items.extend(run.into_iter().chain(search));
    } else {
        items.extend(search.into_iter().chain(run));
    }
    items.extend(found.into_iter().map(|c| Item::History(c.clone())));
    items.truncate(MAX_ITEMS);
    items
}

/// Starts `command` with `shell` and no window, the way Win+R would, in
/// `cwd`. Output goes nowhere: Ctrl+Enter is for commands worth watching.
pub fn run_detached(shell: &OsStr, command: &str, cwd: &Path) -> std::io::Result<()> {
    let stem = Path::new(shell)
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut process = Command::new(shell);
    match stem.as_str() {
        "pwsh" | "powershell" => {
            process.args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                command,
            ]);
        }
        _ => {
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                // cmd parses its own command line; std's quoting would break it.
                process.raw_arg(format!("/d /s /c \"{command}\""));
            }
            #[cfg(not(windows))]
            process.args(["-c", command]);
        }
    }
    process
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        process.creation_flags(CREATE_NO_WINDOW);
    }
    process.spawn().map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn launcher(history: &[&str]) -> Launcher {
        Launcher::open(Sources {
            history: strings(history),
            ..Sources::default()
        })
    }

    const GOOGLE: &str = "https://www.google.com/search?q={query}";

    /// History, programs on PATH, and web search on.
    fn searching(history: &[&str], commands: &[&str]) -> Launcher {
        Launcher::open(Sources {
            history: strings(history),
            commands: commands.iter().map(|s| s.to_string()).collect(),
            search_url: GOOGLE.into(),
            ..Sources::default()
        })
    }

    #[test]
    fn words_that_are_no_command_search_the_web_first() {
        let mut l = searching(&[], &["git"]);
        l.insert("rust borrow checker");
        assert_eq!(
            texts(&l),
            [
                "Search the web for \u{201c}rust borrow checker\u{201d}",
                "rust borrow checker"
            ]
        );
        assert_eq!(
            l.choose(false),
            Some(Action::OpenUrl(
                "https://www.google.com/search?q=rust+borrow+checker".into()
            ))
        );
    }

    #[test]
    fn commands_run_first_and_still_offer_a_search() {
        for query in [
            "git status",
            "Get-Process",
            r"C:\tools\x.exe",
            r".\build.ps1",
            "git.exe log",
        ] {
            let mut l = searching(&[], &["git"]);
            l.insert(query);
            assert_eq!(l.items()[0], Item::Run(query.into()), "{query}");
            assert!(matches!(l.items()[1], Item::Search { .. }), "{query}");
        }
        let mut l = searching(&["mytool --flag"], &[]);
        l.insert("mytool other");
        assert_eq!(l.items()[0], Item::Run("mytool other".into()));
    }

    #[test]
    fn a_question_mark_forces_a_search() {
        let mut l = searching(&["git status"], &["git"]);
        l.insert("? git status");
        assert_eq!(texts(&l), ["Search the web for \u{201c}git status\u{201d}"]);
    }

    #[test]
    fn no_search_for_math_builtins_or_when_disabled() {
        for query in ["2+2", "theme"] {
            let mut l = searching(&[], &[]);
            l.insert(query);
            assert!(
                !l.items().iter().any(|i| matches!(i, Item::Search { .. })),
                "{query}"
            );
        }
        let mut l = launcher(&[]);
        l.insert("rust borrow checker");
        assert_eq!(texts(&l), ["rust borrow checker"]);
    }

    #[test]
    fn search_urls_are_encoded() {
        assert_eq!(
            search_url(GOOGLE, "c++ & ñ?"),
            "https://www.google.com/search?q=c%2B%2B+%26+%C3%B1%3F"
        );
        assert_eq!(
            search_url("https://duckduckgo.com/?q=", "a b"),
            "https://duckduckgo.com/?q=a+b"
        );
    }

    fn with_apps(history: &[&str], apps: &[&str]) -> Launcher {
        Launcher::open(Sources {
            history: strings(history),
            apps: apps
                .iter()
                .map(|n| App {
                    name: n.to_string(),
                    id: format!("id.{n}"),
                    uses: 0,
                })
                .collect(),
            ..Sources::default()
        })
    }

    #[test]
    fn apps_come_before_running_the_query() {
        let mut l = with_apps(&["code ."], &["Visual Studio Code", "Discord"]);
        l.insert("code");
        assert_eq!(texts(&l), ["Visual Studio Code", "code", "code ."]);
        assert_eq!(
            l.choose(false),
            Some(Action::OpenApp("id.Visual Studio Code".into()))
        );
        assert_eq!(
            l.choose(true),
            Some(Action::OpenApp("id.Visual Studio Code".into()))
        );
    }

    #[test]
    fn an_app_named_exactly_like_the_query_replaces_run() {
        let mut l = with_apps(&[], &["Discord"]);
        l.insert("discord");
        assert_eq!(texts(&l), ["Discord"]);
    }

    #[test]
    fn empty_query_lists_history_not_apps() {
        let l = with_apps(&["git status"], &["Discord"]);
        assert_eq!(texts(&l), ["git status"]);
    }

    #[test]
    fn theme_offers_toggle_dark_and_light() {
        let mut l = launcher(&[]);
        l.insert("th");
        assert_eq!(
            l.items()[..3],
            [
                Item::Builtin(Builtin::Theme(Theme::Toggle)),
                Item::Builtin(Builtin::Theme(Theme::Dark)),
                Item::Builtin(Builtin::Theme(Theme::Light)),
            ]
        );
        assert_eq!(l.choose(false), Some(Action::SetTheme(Theme::Toggle)));
        assert_eq!(l.items().len(), 3, "no shell command named theme");
    }

    #[test]
    fn theme_takes_a_mode_in_english_or_spanish() {
        for (query, theme) in [
            ("theme dark", Theme::Dark),
            ("theme l", Theme::Light),
            ("tema oscuro", Theme::Dark),
            ("tema claro", Theme::Light),
        ] {
            let mut l = launcher(&[]);
            l.insert(query);
            assert_eq!(l.choose(false), Some(Action::SetTheme(theme)), "{query}");
        }
        let mut l = launcher(&[]);
        l.insert("t");
        assert!(!l.items().iter().any(|i| matches!(i, Item::Builtin(_))));
    }

    #[test]
    fn set_catalog_keeps_the_query() {
        let mut l = launcher(&[]);
        l.insert("disc");
        let discord = App {
            name: "Discord".into(),
            id: "d".into(),
            uses: 0,
        };
        l.set_catalog(vec![discord.clone()], HashSet::new());
        assert_eq!(l.query(), "disc");
        assert_eq!(l.items()[0], Item::App(discord));
    }

    fn texts(l: &Launcher) -> Vec<&str> {
        l.items().iter().map(Item::text).collect()
    }

    #[test]
    fn empty_query_lists_recent_history() {
        let l = launcher(&["code .", "git status"]);
        assert_eq!(texts(&l), ["code .", "git status"]);
    }

    #[test]
    fn typing_offers_run_then_matches_prefix_first() {
        let mut l = launcher(&["git status", "code C:\\software\\aeon", "vscode", "code ."]);
        l.insert("code");
        assert_eq!(
            texts(&l),
            ["code", "code C:\\software\\aeon", "code .", "vscode"]
        );
        assert_eq!(l.choose(false), Some(Action::Run("code".into())));
    }

    #[test]
    fn exact_history_match_is_not_duplicated() {
        let mut l = launcher(&["code ."]);
        l.insert("code .");
        assert_eq!(l.items(), [Item::History("code .".into())]);
    }

    #[test]
    fn all_terms_must_match_case_insensitively() {
        let mut l = launcher(&["code C:\\software\\aeon", "code C:\\other"]);
        l.insert("AEON code");
        assert_eq!(texts(&l)[1..], ["code C:\\software\\aeon"]);
    }

    #[test]
    fn math_copies_its_result() {
        let mut l = launcher(&["5*3-2 is in history"]);
        l.insert("5*3-2");
        assert_eq!(
            l.items()[0],
            Item::Calc {
                result: "13".into()
            }
        );
        assert_eq!(l.choose(false), Some(Action::Copy("13".into())));
        assert_eq!(l.choose(true), Some(Action::Copy("13".into())));
    }

    #[test]
    fn selection_wraps_and_ctrl_enter_runs_in_terminal() {
        let mut l = launcher(&["a", "b"]);
        l.select_previous();
        assert_eq!(l.choose(true), Some(Action::RunInTerminal("b".into())));
        l.select_next();
        assert_eq!(l.choose(false), Some(Action::Run("a".into())));
    }

    #[test]
    fn tab_completes_and_ctrl_backspace_deletes_a_word() {
        let mut l = launcher(&["code C:\\software\\aeon"]);
        l.insert("aeon");
        l.select_next();
        l.complete();
        assert_eq!(l.query(), "code C:\\software\\aeon");
        l.delete_word();
        assert_eq!(l.query(), "code C:\\software\\");
        l.delete_word();
        assert_eq!(l.query(), "code C:\\");
    }

    #[test]
    fn pasted_newlines_are_dropped() {
        let mut l = launcher(&[]);
        l.insert("git\r\nstatus");
        assert_eq!(l.query(), "gitstatus");
    }

    /// Waits for `path` to appear, written by a detached command.
    fn wait_for(path: &Path) -> Option<String> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            if let Ok(text) = std::fs::read_to_string(path) {
                return Some(text);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        None
    }

    #[cfg(windows)]
    #[test]
    fn runs_detached_through_cmd_and_powershell() {
        let dir = std::env::temp_dir();
        let id = std::process::id();
        let cmd_out = dir.join(format!("aeon-run-cmd-{id}.txt"));
        run_detached(
            OsStr::new("cmd.exe"),
            &format!("echo from cmd> \"{}\"", cmd_out.display()),
            &dir,
        )
        .unwrap();
        assert_eq!(
            wait_for(&cmd_out).as_deref().map(str::trim),
            Some("from cmd")
        );
        let _ = std::fs::remove_file(&cmd_out);

        let ps_out = dir.join(format!("aeon-run-ps-{id}.txt"));
        run_detached(
            OsStr::new("powershell.exe"),
            &format!(
                "Set-Content -Path '{}' -Value 'from powershell'",
                ps_out.display()
            ),
            &dir,
        )
        .unwrap();
        assert_eq!(
            wait_for(&ps_out).as_deref().map(str::trim),
            Some("from powershell")
        );
        let _ = std::fs::remove_file(&ps_out);
    }

    fn entry(id: usize, title: &str, detail: &str) -> Entry {
        Entry {
            id,
            title: title.into(),
            detail: detail.into(),
        }
    }

    fn profile(label: &str, kind: profiles::Kind) -> Profile {
        Profile {
            label: label.into(),
            kind,
            program: "wsl.exe".into(),
            args: Vec::new(),
        }
    }

    #[test]
    fn an_empty_box_lists_sessions_then_history() {
        let l = Launcher::open(Sources {
            history: strings(&["git status"]),
            sessions: vec![entry(0, "pwsh", "aeon"), entry(3, "npm run dev", "web")],
            ..Sources::default()
        });
        assert_eq!(texts(&l), ["pwsh", "npm run dev", "git status"]);
        assert_eq!(l.choose(false), Some(Action::GoToSession(0)));
    }

    #[test]
    fn sessions_tasks_and_profiles_match_by_words() {
        let sources = Sources {
            sessions: vec![entry(2, "pwsh", "aeon"), entry(5, "pwsh", "web")],
            tasks: vec![
                entry(0, "dev", "npm run dev"),
                entry(1, "test", "cargo test"),
            ],
            profiles: vec![
                profile("Ubuntu", profiles::Kind::Wsl),
                profile("build-box", profiles::Kind::Ssh),
            ],
            ..Sources::default()
        };
        let mut l = Launcher::open(sources.clone());
        l.insert("web");
        assert_eq!(l.items()[0], Item::Session(entry(5, "pwsh", "web")));
        let mut l = Launcher::open(sources.clone());
        l.insert("cargo");
        assert_eq!(l.choose(false), Some(Action::RunTask(1)));
        let mut l = Launcher::open(sources.clone());
        l.insert("new wsl");
        assert_eq!(texts(&l)[0], "Ubuntu");
        assert!(matches!(l.choose(false), Some(Action::NewSession(p)) if p.label == "Ubuntu"));
        let mut l = Launcher::open(sources);
        l.insert("ssh");
        assert_eq!(texts(&l)[0], "build-box");
    }

    #[test]
    fn the_menu_lists_only_profiles() {
        let mut l = Launcher::menu(Sources {
            history: strings(&["ubuntu-thing"]),
            profiles: vec![
                profile("Ubuntu", profiles::Kind::Wsl),
                profile("Debian", profiles::Kind::Wsl),
                profile("Aeon", profiles::Kind::Admin),
            ],
            ..Sources::default()
        });
        assert!(l.is_menu());
        assert_eq!(texts(&l), ["Ubuntu", "Debian", "Aeon"]);
        l.insert("admin");
        assert_eq!(texts(&l), ["Aeon"]);
        l.backspace();
        l.select(2);
        assert_eq!(l.selected(), 0, "typing resets the selection");
    }

    #[test]
    fn list_is_capped() {
        let history: Vec<String> = (0..20).map(|i| format!("cmd {i}")).collect();
        let l = Launcher::open(Sources {
            history,
            ..Sources::default()
        });
        assert_eq!(l.items().len(), MAX_ITEMS);
    }
}
