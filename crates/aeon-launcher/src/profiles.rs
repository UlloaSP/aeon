//! What a new session can start: the shells this PC has, each WSL distro,
//! the hosts in `~/.ssh/config`, and Aeon as administrator.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// What the list shows, such as `Ubuntu`.
    pub label: String,
    pub kind: Kind,
    /// The program a session runs; empty for [`Kind::Admin`], which starts
    /// the configured shell in a new, elevated window.
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Shell,
    Wsl,
    Ssh,
    Admin,
}

impl Kind {
    /// The word the list shows beside a profile, and that a query matches.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Shell => "Shell",
            Kind::Wsl => "WSL",
            Kind::Ssh => "SSH",
            Kind::Admin => "Administrator",
        }
    }
}

impl Profile {
    fn new(label: &str, kind: Kind, program: &str, args: &[&str]) -> Self {
        Self {
            label: label.to_owned(),
            kind,
            program: program.to_owned(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
        }
    }
}

/// Every profile this PC has. Reads PATH, the registry, and one small file:
/// call it off the UI thread.
#[cfg(windows)]
pub fn detect() -> Vec<Profile> {
    let mut profiles = Vec::new();
    for (label, exe) in [
        ("PowerShell 7", "pwsh.exe"),
        ("Windows PowerShell", "powershell.exe"),
        ("Command Prompt", "cmd.exe"),
    ] {
        if on_path(exe) {
            profiles.push(Profile::new(label, Kind::Shell, exe, &[]));
        }
    }
    let program_files = std::env::var_os("ProgramFiles")
        .map_or_else(|| PathBuf::from(r"C:\Program Files"), PathBuf::from);
    let git_bash = program_files.join(r"Git\bin\bash.exe");
    if git_bash.is_file() {
        profiles.push(Profile::new(
            "Git Bash",
            Kind::Shell,
            &git_bash.to_string_lossy(),
            &["--login", "-i"],
        ));
    }
    for distro in crate::sys::wsl_distros() {
        profiles.push(Profile::new(
            &distro,
            Kind::Wsl,
            "wsl.exe",
            &["-d", &distro],
        ));
    }
    if on_path("ssh.exe")
        && let Some(home) = std::env::var_os("USERPROFILE")
    {
        let config = Path::new(&home).join(".ssh").join("config");
        let text = std::fs::read_to_string(config).unwrap_or_default();
        for host in ssh_hosts(&text) {
            profiles.push(Profile::new(&host, Kind::Ssh, "ssh.exe", &[&host]));
        }
    }
    profiles.push(Profile::new("Aeon as administrator", Kind::Admin, "", &[]));
    profiles
}

fn on_path(exe: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(exe).is_file()))
}

/// The hosts an SSH config names, without patterns (`*`, `?`, `!`), in file
/// order and once each.
pub fn ssh_hosts(config: &str) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for line in config.lines() {
        let line = line.trim();
        let Some((keyword, rest)) = line
            .split_once(|c: char| c.is_whitespace() || c == '=')
            .map(|(k, r)| {
                (
                    k,
                    r.trim_start_matches(|c: char| c.is_whitespace() || c == '='),
                )
            })
        else {
            continue;
        };
        if !keyword.eq_ignore_ascii_case("host") {
            continue;
        }
        for name in rest.split_whitespace() {
            let name = name.trim_matches('"');
            if !name.is_empty()
                && !name.contains(['*', '?', '!'])
                && !hosts.iter().any(|h| h == name)
            {
                hosts.push(name.to_owned());
            }
        }
    }
    hosts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_hosts_skip_patterns_and_repeats() {
        let config = "\
# work
Host build-box staging
    HostName 10.0.0.4
    User me
Host *.internal !secret
host=pi
Host \"quoted\"
Host build-box
Match host foo
";
        assert_eq!(ssh_hosts(config), ["build-box", "staging", "pi", "quoted"]);
        assert!(ssh_hosts("").is_empty());
    }
}
