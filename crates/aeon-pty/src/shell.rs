//! What Aeon changes about the shells it knows. Only applies when the user
//! gave no arguments, or the ones Aeon itself starts the shell with:
//! explicit arguments mean they want the shell as is.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::Command;

const POWERSHELL_INIT: &str = include_str!("aeon.ps1");

/// Scripts for bash and zsh, which read files rather than a command: the
/// WSL launcher, bash's init file, the colouring both share, and zsh's dot
/// files.
const UNIX_FILES: [(&str, &str); 6] = [
    ("aeon.sh", include_str!("unix/aeon.sh")),
    ("aeon.bash", include_str!("unix/aeon.bash")),
    ("aeon-paint.sh", include_str!("unix/aeon-paint.sh")),
    ("zsh/.zshenv", include_str!("unix/zshenv")),
    ("zsh/.zprofile", include_str!("unix/zprofile")),
    ("zsh/.zshrc", include_str!("unix/zshrc")),
];

/// cmd's prompt as Aeon's: the chevron, after the blank line cmd leaves
/// after every command; the header shows the folder, which it reports (OSC
/// 9;9). cmd marks prompts for command blocks (OSC 133 A and B), but cannot
/// mark when a command runs or how it ended (the terminal takes Enter as the
/// start), so its chevron is always green.
const CMD_PROMPT: &str = "$e]133;A$e\\$e]9;9;$P$e\\$e[1;32m\u{276f}$e[0m $e]133;B$e\\";

/// What a shell is, as the sidebar, header, and window title name it:
/// "PowerShell", "WSL · kali-linux", "Git Bash", "SSH · host". Takes the
/// command as configured, before `integrate`.
pub fn describe(command: &Command) -> String {
    let program = command.program.to_string_lossy().to_lowercase();
    let stem = Path::new(&program)
        .file_stem()
        .map(OsStr::to_string_lossy)
        .unwrap_or_default()
        .into_owned();
    let args: Vec<String> = command
        .args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let after = |flags: &[&str]| {
        let at = args.iter().position(|a| flags.contains(&a.as_str()))?;
        args.get(at + 1).cloned()
    };
    match stem.as_str() {
        "pwsh" => "PowerShell".to_owned(),
        "powershell" => "Windows PowerShell".to_owned(),
        "cmd" => "Command Prompt".to_owned(),
        "wsl" => match after(&["-d", "--distribution"]) {
            Some(distro) => format!("WSL \u{b7} {distro}"),
            None => "WSL".to_owned(),
        },
        "bash" if program.contains("git") => "Git Bash".to_owned(),
        "ssh" => match args.iter().rev().find(|a| !a.starts_with('-')) {
            Some(host) => format!("SSH \u{b7} {host}"),
            None => "SSH".to_owned(),
        },
        "nu" => "Nushell".to_owned(),
        _ => Path::new(&command.program)
            .file_stem()
            .map_or_else(|| "Shell".to_owned(), |s| s.to_string_lossy().into_owned()),
    }
}

/// Sets up shells for Aeon. PowerShell loads the user's profiles itself (so
/// it can skip them on request), then takes Aeon's history file and prompt.
/// bash and zsh (WSL, Git Bash) load the user's files first, then Aeon's
/// prompt. All of them report their working
/// directory for the header. `run` is a first command for the session, which
/// then stays open (the launcher's Ctrl+Enter); only PowerShell and cmd take
/// one.
pub fn integrate(mut command: Command, run: Option<&str>) -> Command {
    let program = command.program.to_string_lossy().to_lowercase();
    let stem = Path::new(&program)
        .file_stem()
        .map(OsStr::to_string_lossy)
        .unwrap_or_default()
        .into_owned();
    let args: Vec<&str> = command.args.iter().filter_map(|a| a.to_str()).collect();
    let bare = command.args.is_empty();
    match stem.as_str() {
        // A distro picked by name is still the plain shell.
        "wsl" if bare || matches!(args[..], ["-d", _]) => {
            match unix_scripts().and_then(|dir| wsl_path(&dir.join("aeon.sh"))) {
                Some(script) => command.args(["-e", "sh", &script]),
                None => command,
            }
        }
        // Git Bash, not System32's bash.exe, which is WSL's.
        "bash" if program.contains("git") && (bare || args == ["--login", "-i"]) => {
            let Some(dir) = unix_scripts() else {
                return command;
            };
            let script = dir.join("aeon.bash").to_string_lossy().replace('\\', "/");
            command.args.clear();
            command
                .args(["--init-file", &script, "-i"])
                .env("AEON_SHELL_LOGIN", "1")
        }
        _ if !bare => command,
        "pwsh" | "powershell" => {
            let mut script = POWERSHELL_INIT.to_owned();
            if let Some(run) = run {
                // Shown and marked as if typed at a prompt, so it becomes a
                // command block with its time and result like any other.
                let shown = run.replace('\'', "''");
                // Run as a typed line is, so its statements become blocks too.
                script.push_str(&format!(
                    "\n$e = [char]27; $global:AeonChevron = \"$e[1;32m$([char]0x276F)$e[0m\"; [Console]::Write(\"$e]133;A$e\\$global:AeonChevron $e]133;B$e\\\" + (__aeon_colored '{shown}') + \"`r`n\")\nInvoke-Expression (__aeon_line '{shown}')"
                ));
            }
            command
                .args(["-NoLogo", "-NoProfile", "-NoExit", "-EncodedCommand"])
                .arg(encoded_command(&script))
        }
        "cmd" => {
            let command = command.env("PROMPT", CMD_PROMPT);
            match run {
                Some(run) => command.args(["/k", run]),
                None => command,
            }
        }
        _ => command,
    }
}

/// Where the bash and zsh scripts are. Written once per version of them,
/// to a folder named after their contents, so Aeon builds that differ never
/// change a file a running shell reads.
fn unix_scripts() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        // FNV-1a: a name for these exact contents.
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for (name, text) in UNIX_FILES {
            for byte in name.bytes().chain(text.bytes()) {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
            }
        }
        let base = std::env::var_os("LOCALAPPDATA").map_or_else(std::env::temp_dir, PathBuf::from);
        let dir = base.join("aeon").join("shell").join(format!("{hash:016x}"));
        for (name, text) in UNIX_FILES {
            let path = dir.join(name);
            if !path.is_file() {
                std::fs::create_dir_all(path.parent()?).ok()?;
                // A CRLF checkout must not reach a Linux shell.
                std::fs::write(&path, text.replace("\r\n", "\n")).ok()?;
            }
        }
        Some(dir)
    })
    .as_deref()
}

/// A Windows path as WSL mounts it, such as `/mnt/c/Users`. `None` for paths
/// not on a drive letter.
fn wsl_path(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    let (drive, rest) = text.split_once(":\\")?;
    (drive.len() == 1 && drive.chars().all(|c| c.is_ascii_alphabetic())).then(|| {
        format!(
            "/mnt/{}/{}",
            drive.to_ascii_lowercase(),
            rest.replace('\\', "/")
        )
    })
}

/// PowerShell's `-EncodedCommand` format: base64 of the UTF-16LE script. It
/// sidesteps every quoting rule of the command line.
fn encoded_command(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64(&bytes)
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc_4648_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected);
        }
    }

    #[test]
    fn shells_are_named_as_people_know_them() {
        let cases = [
            (
                Command::new(r"C:\Program Files\PowerShell\7\pwsh.exe"),
                "PowerShell",
            ),
            (Command::new("powershell.exe"), "Windows PowerShell"),
            (Command::new("cmd.exe"), "Command Prompt"),
            (Command::new("wsl.exe"), "WSL"),
            (
                Command::new("wsl.exe").args(["-d", "kali-linux"]),
                "WSL \u{b7} kali-linux",
            ),
            (
                Command::new(r"C:\Program Files\Git\bin\bash.exe").args(["--login", "-i"]),
                "Git Bash",
            ),
            (
                Command::new("ssh.exe").args(["-p", "22", "pi"]),
                "SSH \u{b7} pi",
            ),
            (Command::new(r"C:\tools\fish.exe"), "fish"),
        ];
        for (command, name) in cases {
            assert_eq!(describe(&command), name);
        }
    }

    #[test]
    fn powershell_skips_the_profile() {
        let command = integrate(
            Command::new(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            None,
        );
        assert_eq!(
            command.args[..4],
            ["-NoLogo", "-NoProfile", "-NoExit", "-EncodedCommand"]
        );
        let windows = integrate(Command::new("powershell.exe"), None);
        assert!(windows.args.iter().any(|a| a == "-NoProfile"));
    }

    #[test]
    fn cmd_reports_its_directory() {
        let command = integrate(Command::new(r"C:\WINDOWS\system32\cmd.exe"), None);
        assert!(command.args.is_empty());
        assert_eq!(command.env, [("PROMPT".into(), CMD_PROMPT.into())]);
    }

    #[test]
    fn a_first_command_runs_and_the_session_stays_open() {
        let cmd = integrate(Command::new("cmd.exe"), Some("git status"));
        assert_eq!(cmd.args, ["/k", "git status"]);
        let pwsh = integrate(Command::new("pwsh.exe"), Some("git status"));
        assert_ne!(pwsh.args, integrate(Command::new("pwsh.exe"), None).args);
        assert!(pwsh.args.iter().any(|a| a == "-NoExit"));
    }

    #[test]
    fn explicit_args_and_unknown_shells_are_untouched() {
        let pwsh = Command::new("pwsh.exe").arg("-Login");
        assert_eq!(integrate(pwsh.clone(), Some("x")), pwsh);
        let wsl = Command::new("wsl.exe").args(["-d", "Ubuntu", "-u", "root"]);
        assert_eq!(integrate(wsl.clone(), None), wsl);
        let system_bash = Command::new(r"C:\Windows\System32\bash.exe");
        assert_eq!(integrate(system_bash.clone(), None), system_bash);
        let nu = Command::new("nu.exe");
        assert_eq!(integrate(nu.clone(), None), nu);
    }

    #[test]
    fn wsl_starts_the_account_shell_through_aeons_launcher() {
        let wsl = integrate(Command::new("wsl.exe").args(["-d", "Ubuntu"]), None);
        let args: Vec<_> = wsl.args.iter().map(|a| a.to_string_lossy()).collect();
        assert_eq!(args[..4], ["-d", "Ubuntu", "-e", "sh"]);
        assert!(args[4].starts_with("/mnt/") && args[4].ends_with("/aeon.sh"));
        let default = integrate(Command::new("wsl.exe"), None);
        assert_eq!(default.args[..2], ["-e", "sh"]);
    }

    #[test]
    fn git_bash_reads_aeons_init_file_as_a_login_shell() {
        let bash = integrate(
            Command::new(r"C:\Program Files\Git\bin\bash.exe").args(["--login", "-i"]),
            None,
        );
        assert_eq!(bash.args[0], "--init-file");
        assert!(bash.args[1].to_string_lossy().ends_with("/aeon.bash"));
        assert_eq!(bash.args[2], "-i");
        assert_eq!(bash.env, [("AEON_SHELL_LOGIN".into(), "1".into())]);
        let script = std::path::Path::new(bash.args[1].to_str().unwrap());
        assert!(std::fs::read_to_string(script).is_ok_and(|s| !s.contains('\r')));
    }

    #[test]
    fn windows_paths_map_to_wsl_mounts() {
        assert_eq!(
            wsl_path(Path::new(r"C:\Users\me\AppData\aeon.sh")).as_deref(),
            Some("/mnt/c/Users/me/AppData/aeon.sh")
        );
        assert_eq!(wsl_path(Path::new(r"\\server\share\x")), None);
    }
}
