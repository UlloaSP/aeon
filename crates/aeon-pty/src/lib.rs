//! Spawns a shell attached to a Windows pseudo console (ConPTY).
//!
//! The PTY hands out two blocking pipes: read shell output from
//! [`Pty::take_reader`] on a dedicated thread, write keyboard input to
//! [`Pty::take_writer`]. Never read on the UI thread.

use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(windows)]
mod conpty;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
}

impl Command {
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I: IntoIterator<Item = S>, S: Into<OsString>>(mut self, args: I) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }
}

pub struct Pty {
    #[cfg(windows)]
    inner: conpty::ConPty,
}

impl Pty {
    #[cfg(windows)]
    pub fn spawn(command: &Command, size: PtySize) -> io::Result<Self> {
        conpty::ConPty::spawn(command, size).map(|inner| Self { inner })
    }

    #[cfg(not(windows))]
    pub fn spawn(_command: &Command, _size: PtySize) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "aeon-pty only supports Windows ConPTY",
        ))
    }

    /// Shell output. `None` after the first call.
    pub fn take_reader(&mut self) -> Option<File> {
        #[cfg(windows)]
        return self.inner.reader.take();
        #[cfg(not(windows))]
        None
    }

    /// Shell input. `None` after the first call.
    pub fn take_writer(&mut self) -> Option<File> {
        #[cfg(windows)]
        return self.inner.writer.take();
        #[cfg(not(windows))]
        None
    }

    pub fn resize(&self, size: PtySize) -> io::Result<()> {
        #[cfg(windows)]
        return self.inner.resize(size);
        #[cfg(not(windows))]
        {
            let _ = size;
            Ok(())
        }
    }

    /// Blocks until the shell exits and returns its exit code.
    pub fn wait(&self) -> io::Result<u32> {
        #[cfg(windows)]
        return self.inner.wait();
        #[cfg(not(windows))]
        Err(io::ErrorKind::Unsupported.into())
    }
}

/// The best interactive shell on this machine: PowerShell 7, then Windows
/// PowerShell, then `%COMSPEC%`.
pub fn default_shell() -> Command {
    ["pwsh.exe", "powershell.exe"]
        .into_iter()
        .find_map(find_in_path)
        .map(Command::new)
        .or_else(|| std::env::var_os("COMSPEC").map(Command::new))
        .unwrap_or_else(|| Command::new("cmd.exe"))
}

fn find_in_path(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(exe))
        .find(|candidate| Path::new(candidate).is_file())
}

/// Joins a program and its arguments into one Windows command line, quoting
/// the way `CommandLineToArgvW` (and the MSVC CRT) parse it back.
pub fn command_line(command: &Command) -> OsString {
    let mut line = OsString::new();
    for (i, part) in std::iter::once(&command.program)
        .chain(&command.args)
        .enumerate()
    {
        if i > 0 {
            line.push(" ");
        }
        quote_into(&mut line, &part.to_string_lossy());
    }
    line
}

fn quote_into(out: &mut OsString, arg: &str) {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        out.push(arg);
        return;
    }
    let mut quoted = String::with_capacity(arg.len() + 2);
    quoted.push('"');
    let mut backslashes = 0;
    for ch in arg.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                quoted.push(ch);
                backslashes = 0;
            }
        }
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    out.push(quoted);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(program: &str, args: &[&str]) -> String {
        command_line(&Command::new(program).args(args))
            .into_string()
            .unwrap()
    }

    #[test]
    fn plain_args_are_not_quoted() {
        assert_eq!(line("cmd.exe", &["/c", "echo"]), "cmd.exe /c echo");
    }

    #[test]
    fn spaces_and_empty_args_are_quoted() {
        assert_eq!(
            line(r"C:\Program Files\PowerShell\7\pwsh.exe", &["-NoLogo", ""]),
            r#""C:\Program Files\PowerShell\7\pwsh.exe" -NoLogo """#
        );
    }

    #[test]
    fn quotes_and_trailing_backslashes_are_escaped() {
        assert_eq!(line("x", &[r#"a "b""#]), r#"x "a \"b\"""#);
        assert_eq!(line("x", &[r"dir with space\"]), r#"x "dir with space\\""#);
    }
}
