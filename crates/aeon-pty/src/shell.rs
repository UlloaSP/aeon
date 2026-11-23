//! What Aeon changes about the shells it knows. Only applies when the user
//! gave no arguments: explicit arguments mean they want the shell as is.

use std::ffi::OsStr;
use std::path::Path;

use crate::Command;

const POWERSHELL_INIT: &str = include_str!("aeon.ps1");

/// cmd's own prompt (`C:\path>`) plus an OSC 9;9 working-directory report.
const CMD_PROMPT: &str = r"$e]9;9;$P$e\$P$G";

/// Sets up PowerShell and cmd for Aeon. PowerShell starts without the user's
/// profile, with Aeon's history file and prompt. Both report their working
/// directory for the context bar. `run` is a first command for the session,
/// which then stays open (the launcher's Ctrl+Enter).
pub fn integrate(command: Command, run: Option<&str>) -> Command {
    if !command.args.is_empty() {
        return command;
    }
    let stem = Path::new(&command.program)
        .file_stem()
        .map(OsStr::to_string_lossy)
        .unwrap_or_default()
        .to_lowercase();
    match stem.as_str() {
        "pwsh" | "powershell" => {
            let mut script = POWERSHELL_INIT.to_owned();
            if let Some(run) = run {
                script.push('\n');
                script.push_str(run);
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
        let wsl = Command::new("wsl.exe");
        assert_eq!(integrate(wsl.clone(), Some("x")), wsl);
    }
}
