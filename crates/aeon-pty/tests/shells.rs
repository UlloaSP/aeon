//! Shells started with Aeon's integration, through a real pseudo console.
#![cfg(windows)]

use std::io::{Read, Write};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use aeon_pty::{Command, Pty, PtySize, integrate};

struct Session {
    _pty: Pty,
    input: std::fs::File,
    output: Receiver<Vec<u8>>,
    seen: String,
    /// What the last `expect` consumed.
    last: String,
}

impl Session {
    fn start(command: Command) -> Self {
        let mut pty = Pty::spawn(
            &command,
            PtySize {
                rows: 24,
                cols: 100,
            },
        )
        .unwrap();
        let mut reader = pty.take_reader().unwrap();
        let input = pty.take_writer().unwrap();
        let (tx, output) = channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n @ 1..) = reader.read(&mut buf) {
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Self {
            _pty: pty,
            input,
            output,
            seen: String::new(),
            last: String::new(),
        }
    }

    /// Waits for `text` in what the shell printed since the last match.
    fn expect(&mut self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !self.seen.contains(text) {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.output.recv_timeout(left) {
                Ok(chunk) => self.seen.push_str(&String::from_utf8_lossy(&chunk)),
                Err(_) => panic!("no {text:?} in {:?}", self.seen),
            }
        }
        let end = self.seen.find(text).expect("just found") + text.len();
        self.last = self.seen.drain(..end).collect();
    }

    /// Waits for `text`, failing if `absent` shows up before it.
    fn expect_without(&mut self, text: &str, absent: &str) {
        self.expect(text);
        // `expect` drained everything up to the match; check what it saw.
        assert!(!self.last.contains(absent), "{absent:?} in {:?}", self.last);
    }

    fn type_line(&mut self, line: &str) {
        self.input
            .write_all(format!("{line}\r").as_bytes())
            .unwrap();
    }
}

#[test]
fn powershell_runs_each_statement_of_a_line_as_its_own_block() {
    let Some(pwsh) = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("pwsh.exe"))
            .find(|p| p.is_file())
    }) else {
        return;
    };
    let command = integrate(Command::new(pwsh), None)
        .env("AEON_NO_PROFILE", "1")
        .cwd(std::env::temp_dir());
    let mut s = Session::start(command);
    s.expect("\x1b]133;B\x1b\\");
    s.type_line("echo one; cmd /c exit 3; echo two");
    s.expect("\x1b]133;C;cmdline_url=echo%20one\x1b\\");
    s.expect("\x1b]133;D;0\x1b\\");
    s.expect("\x1b]133;C;cmdline_url=cmd%20%2Fc%20exit%203\x1b\\");
    s.expect("\x1b]133;D;3\x1b\\");
    s.expect("\x1b]133;C;cmdline_url=echo%20two\x1b\\");
    s.expect("\x1b]133;D;0\x1b\\");
    // Each statement is formatted on its own: objects after a listing still
    // make a table, not a list.
    s.type_line("Get-Item . | Select-Object Name; [pscustomobject]@{ A = 1; B = 2 }");
    s.expect("\x1b]133;D;0\x1b\\");
    s.expect_without("\x1b]133;D;0\x1b\\", "A : 1");
    // `&&` is one statement, and `$?` says what the statement before did.
    s.type_line("echo a && echo b");
    s.expect("\x1b]133;C\x1b\\");
    s.type_line("cmd /c exit 1; echo $?");
    s.expect("\x1b]133;D;1\x1b\\");
    s.expect("False");
    // Ctrl+C stops the rest of the line.
    s.type_line("Start-Sleep 20; echo after");
    s.expect("cmdline_url=Start-Sleep%2020");
    s.input.write_all(b"\x03").unwrap();
    s.expect_without("\x1b]133;B\x1b\\", "after");
    s.type_line("exit");
}

#[test]
fn git_bash_marks_commands_and_reports_its_folder() {
    let bash = r"C:\Program Files\Git\bin\bash.exe";
    // CI's Windows runners have Git for Windows; a machine without it has
    // nothing to test.
    if !std::path::Path::new(bash).is_file() {
        return;
    }
    let dir = std::env::temp_dir();
    let command = integrate(Command::new(bash).args(["--login", "-i"]), None).cwd(&dir);
    let mut s = Session::start(command);
    s.expect("\x1b]133;B\x1b\\");
    s.type_line("false");
    s.expect("\x1b]133;C\x1b\\");
    s.expect("\x1b]133;D;1\x1b\\");
    s.type_line("cd /c/Windows");
    s.expect("\x1b]9;9;\"C:\\Windows\"");
    s.type_line("exit");
}
