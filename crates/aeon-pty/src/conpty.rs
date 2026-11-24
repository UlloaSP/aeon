use std::ffi::{OsString, c_void};
use std::fs::File;
use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, OwnedHandle, RawHandle};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, GetProcessId, INFINITE,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

use crate::{Command, PtySize, command_line};

pub(crate) struct ConPty {
    hpcon: HPCON,
    process: OwnedHandle,
    pub(crate) reader: Option<File>,
    pub(crate) writer: Option<File>,
}

// SAFETY: HPCON is an opaque kernel handle; ConPTY calls on it are thread-safe.
unsafe impl Send for ConPty {}

fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn check_hr(hr: i32) -> io::Result<()> {
    if hr < 0 {
        Err(io::Error::from_raw_os_error(hr))
    } else {
        Ok(())
    }
}

fn coord(size: PtySize) -> COORD {
    COORD {
        X: size.cols.max(1) as i16,
        Y: size.rows.max(1) as i16,
    }
}

/// An anonymous pipe as (read end, write end).
fn pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let (mut read, mut write): (HANDLE, HANDLE) = (null_mut(), null_mut());
    // SAFETY: both out-pointers are valid; on success each handle is owned once.
    unsafe {
        check(CreatePipe(&mut read, &mut write, null(), 0))?;
        Ok((
            OwnedHandle::from_raw_handle(read as RawHandle),
            OwnedHandle::from_raw_handle(write as RawHandle),
        ))
    }
}

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

/// Owns a `PROC_THREAD_ATTRIBUTE_LIST` buffer and deletes it on drop.
struct AttributeList(Vec<u8>);

impl AttributeList {
    fn with_pseudoconsole(hpcon: &HPCON) -> io::Result<Self> {
        let mut size = 0usize;
        // SAFETY: a null list with a size out-pointer only reports the size;
        // this call is documented to fail with ERROR_INSUFFICIENT_BUFFER.
        unsafe { InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size) };
        let mut list = Self(vec![0u8; size]);
        // SAFETY: the buffer is `size` bytes, as requested above. `hpcon`
        // outlives the list because ConPty owns both and the list is dropped
        // right after CreateProcessW.
        unsafe {
            check(InitializeProcThreadAttributeList(
                list.as_ptr(),
                1,
                0,
                &mut size,
            ))?;
            check(UpdateProcThreadAttribute(
                list.as_ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                *hpcon as *const c_void,
                size_of::<HPCON>(),
                null_mut(),
                null(),
            ))?;
        }
        Ok(list)
    }

    fn as_ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.0.as_mut_ptr().cast()
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        // SAFETY: the list was initialized in `with_pseudoconsole`.
        unsafe { DeleteProcThreadAttributeList(self.as_ptr()) };
    }
}

impl ConPty {
    pub(crate) fn spawn(command: &Command, size: PtySize) -> io::Result<Self> {
        let (input_read, input_write) = pipe()?;
        let (output_read, output_write) = pipe()?;

        let mut hpcon: HPCON = 0 as HPCON;
        // SAFETY: the pipe handles are valid. ConPTY duplicates them, so our
        // copies of the PTY-side ends are dropped at the end of this function.
        unsafe {
            check_hr(CreatePseudoConsole(
                coord(size),
                input_read.as_raw_handle_value(),
                output_write.as_raw_handle_value(),
                0,
                &mut hpcon,
            ))?;
        }

        let spawned = Self::create_process(command, &hpcon);
        let process = match spawned {
            Ok(process) => process,
            Err(e) => {
                // SAFETY: created above and not shared with anyone yet.
                unsafe { ClosePseudoConsole(hpcon) };
                return Err(e);
            }
        };

        Ok(Self {
            hpcon,
            process,
            reader: Some(File::from(output_read)),
            writer: Some(File::from(input_write)),
        })
    }

    fn create_process(command: &Command, hpcon: &HPCON) -> io::Result<OwnedHandle> {
        let mut attributes = AttributeList::with_pseudoconsole(hpcon)?;
        let mut cmdline = wide(&command_line(command));
        let cwd = command.cwd.as_ref().map(|d| wide(d.as_os_str()));
        let env = (!command.env.is_empty()).then(|| environment_block(&command.env));

        // SAFETY: STARTUPINFOEXW and PROCESS_INFORMATION are plain C structs
        // for which all-zero is a valid initial state.
        let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.lpAttributeList = attributes.as_ptr();
        // Without this, a child of a process whose stdio is redirected (a test
        // harness, a launcher) writes to those handles instead of the console.
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
        startup.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
        startup.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
        // SAFETY: see above.
        let mut info: PROCESS_INFORMATION = unsafe { zeroed() };

        // SAFETY: every pointer is valid for the duration of the call;
        // `cmdline` is mutable as CreateProcessW requires.
        unsafe {
            check(CreateProcessW(
                null(),
                cmdline.as_mut_ptr(),
                null(),
                null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                env.as_ref().map_or(null(), |e| e.as_ptr().cast()),
                cwd.as_ref().map_or(null(), |d| d.as_ptr()),
                &startup.StartupInfo,
                &mut info,
            ))?;
            drop(OwnedHandle::from_raw_handle(info.hThread as RawHandle));
            Ok(OwnedHandle::from_raw_handle(info.hProcess as RawHandle))
        }
    }

    pub(crate) fn resize(&self, size: PtySize) -> io::Result<()> {
        // SAFETY: `hpcon` is live until drop.
        check_hr(unsafe { ResizePseudoConsole(self.hpcon, coord(size)) })
    }

    pub(crate) fn pid(&self) -> u32 {
        // SAFETY: `process` is a live handle owned by self.
        unsafe { GetProcessId(self.process.as_raw_handle_value()) }
    }

    pub(crate) fn wait(&self) -> io::Result<u32> {
        let process = self.process.as_raw_handle_value();
        // SAFETY: `process` is a live handle owned by self.
        unsafe {
            if WaitForSingleObject(process, INFINITE) != WAIT_OBJECT_0 {
                return Err(io::Error::last_os_error());
            }
            let mut code = 0u32;
            check(GetExitCodeProcess(process, &mut code))?;
            Ok(code)
        }
    }
}

/// Aeon's environment with `overrides` applied, as the sorted, double-NUL
/// terminated UTF-16 block CreateProcessW expects. Names compare without case,
/// as Windows does.
fn environment_block(overrides: &[(OsString, OsString)]) -> Vec<u16> {
    let key = |k: &OsString| k.to_string_lossy().to_uppercase();
    let mut vars: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(k, _)| !overrides.iter().any(|(o, _)| key(o) == key(k)))
        .chain(overrides.iter().cloned())
        .collect();
    vars.sort_by_key(|(k, _)| key(k));

    let mut block = Vec::new();
    for (k, v) in &vars {
        block.extend(k.encode_wide());
        block.push(u16::from(b'='));
        block.extend(v.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

impl Drop for ConPty {
    fn drop(&mut self) {
        // Closing the pseudo console ends the shell and breaks the output pipe,
        // which is what stops the reader thread.
        // SAFETY: `hpcon` is live and closed exactly once.
        unsafe { ClosePseudoConsole(self.hpcon) };
    }
}

trait RawHandleValue {
    fn as_raw_handle_value(&self) -> HANDLE;
}

impl RawHandleValue for OwnedHandle {
    fn as_raw_handle_value(&self) -> HANDLE {
        std::os::windows::io::AsRawHandle::as_raw_handle(self) as HANDLE
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    /// Spawns `command` and returns the PTY once its output contains `marker`.
    fn spawn_until(command: &Command, marker: &str) -> (ConPty, String) {
        let mut pty = ConPty::spawn(
            command,
            PtySize {
                rows: 24,
                cols: 120,
            },
        )
        .unwrap();
        let mut reader = pty.reader.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n @ 1..) = reader.read(&mut buf) {
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });

        // ConPTY discards output it has not flushed when the console closes,
        // so wait for the text itself rather than for the process to exit.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut out = String::new();
        while !out.contains(marker) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match rx.recv_timeout(left) {
                Ok(chunk) => out.push_str(&String::from_utf8_lossy(&chunk)),
                Err(_) => panic!("no {marker:?} in output: {out:?}"),
            }
        }
        (pty, out)
    }

    #[test]
    fn runs_a_command_and_reads_its_output() {
        let command = Command::new("cmd.exe").args(["/d", "/c", "echo aeon-pty-ok"]);
        let (pty, _) = spawn_until(&command, "aeon-pty-ok");
        assert_eq!(pty.wait().unwrap(), 0);
    }

    #[test]
    fn env_overrides_reach_the_child() {
        let command = Command::new("cmd.exe")
            .args(["/d", "/c", "echo [%AEON_TEST_VAR%]"])
            .env("AEON_TEST_VAR", "from-aeon");
        let (pty, _) = spawn_until(&command, "[from-aeon]");
        assert_eq!(pty.wait().unwrap(), 0);
    }

    #[test]
    fn integrated_powershell_marks_commands_and_their_exit() {
        use std::io::Write;
        let Some(program) = ["pwsh.exe", "powershell.exe"]
            .into_iter()
            .find_map(crate::find_in_path)
        else {
            return;
        };
        let appdata = std::env::temp_dir().join(format!("aeon-test-marks-{}", std::process::id()));
        let command = crate::integrate(Command::new(program), None)
            .cwd(env!("CARGO_MANIFEST_DIR"))
            .env("APPDATA", &appdata);
        let mut pty = ConPty::spawn(
            &command,
            PtySize {
                rows: 24,
                cols: 120,
            },
        )
        .unwrap();
        let mut reader = pty.reader.take().unwrap();
        let mut writer = pty.writer.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n @ 1..) = reader.read(&mut buf) {
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut out = String::new();
        let wait_for = |marker: &str, out: &mut String| {
            while !out.contains(marker) {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                match rx.recv_timeout(left) {
                    Ok(chunk) => out.push_str(&String::from_utf8_lossy(&chunk)),
                    Err(_) => panic!("no {marker:?} in output: {out:?}"),
                }
            }
        };
        wait_for("\x1b]133;B", &mut out);
        writer.write_all(b"cmd /c exit 3\r").unwrap();
        wait_for("\x1b]133;D;3", &mut out);
        assert!(out.contains("\x1b]133;C"), "{out:?}");
        let _ = std::fs::remove_dir_all(&appdata);
    }

    #[test]
    fn integrated_powershell_reports_its_directory() {
        let Some(program) = ["pwsh.exe", "powershell.exe"]
            .into_iter()
            .find_map(crate::find_in_path)
        else {
            return;
        };
        // The script writes its history directory under APPDATA; keep that
        // out of the developer's real profile.
        let appdata = std::env::temp_dir().join(format!("aeon-test-{}", std::process::id()));
        // Not the temp dir: on CI it is an 8.3 short path the shell may expand.
        let cwd = env!("CARGO_MANIFEST_DIR");
        let command = crate::integrate(Command::new(program), None)
            .cwd(cwd)
            .env("APPDATA", &appdata);
        let (_pty, _) = spawn_until(&command, &format!("\x1b]9;9;\"{cwd}\""));
        let _ = std::fs::remove_dir_all(&appdata);
    }
}
