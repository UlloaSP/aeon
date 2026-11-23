//! Win32 for the launcher: the global hotkey, the clipboard, and the
//! one-resident-per-session lock. The only `unsafe` code in this crate.

use std::ptr::{copy_nonoverlapping, null, null_mut};
use std::sync::mpsc;

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HANDLE, POINT};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MOD_NOREPEAT, RegisterHotKey};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetMessageW, MSG, WM_HOTKEY};

use crate::Hotkey;

const CF_UNICODETEXT: u32 = 13;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Registers `hotkey` for the whole session and calls `on_press` from a
/// dedicated thread on every press. Fails when another app already owns it.
///
/// The hotkey belongs to the thread that registered it and arrives as a
/// thread message, which winit's loop would drop, so it gets its own loop.
pub fn listen(hotkey: Hotkey, on_press: impl Fn() + Send + 'static) -> Result<(), String> {
    let (registered, result) = mpsc::channel();
    std::thread::Builder::new()
        .name("hotkey".into())
        .spawn(move || {
            // SAFETY: a null window binds the hotkey to this thread's queue.
            let ok = unsafe {
                RegisterHotKey(null_mut(), 1, hotkey.modifiers | MOD_NOREPEAT, hotkey.key)
            };
            let _ = registered.send(ok != 0);
            if ok == 0 {
                return;
            }
            // SAFETY: MSG is plain data; GetMessageW fills it.
            let mut msg: MSG = unsafe { std::mem::zeroed() };
            // SAFETY: `msg` is valid for writes; a null window reads this
            // thread's messages, which is where WM_HOTKEY arrives.
            while unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) } > 0 {
                if msg.message == WM_HOTKEY {
                    on_press();
                }
            }
        })
        .map_err(|e| e.to_string())?;
    match result.recv() {
        Ok(true) => Ok(()),
        _ => Err(format!("{hotkey} is already used by another app")),
    }
}

/// Returns `false` when another process already holds `name`. The lock lasts
/// until this process exits.
pub fn single_instance(name: &str) -> bool {
    let name = wide(name);
    // SAFETY: `name` is NUL-terminated. The handle is deliberately leaked so
    // the mutex lives as long as the process.
    unsafe {
        let handle: HANDLE = CreateMutexW(null(), 0, name.as_ptr());
        !handle.is_null() && GetLastError() != ERROR_ALREADY_EXISTS
    }
}

/// The mouse position in physical screen pixels, to open on the monitor
/// the user is working on.
pub fn cursor_position() -> Option<(i32, i32)> {
    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: `point` is valid for writes.
    (unsafe { GetCursorPos(&mut point) } != 0).then_some((point.x, point.y))
}

/// Puts `text` on the clipboard. `owner` must be a window of this process:
/// with no owner, Windows makes `SetClipboardData` fail.
pub fn set_clipboard(owner: isize, text: &str) -> bool {
    let text = wide(text);
    let bytes = text.len() * 2;
    // SAFETY: the clipboard is opened and closed on every path. Ownership of
    // the global allocation passes to the clipboard on success.
    unsafe {
        if OpenClipboard(owner as _) == 0 {
            return false;
        }
        let mut ok = false;
        if EmptyClipboard() != 0 {
            let memory = GlobalAlloc(GMEM_MOVEABLE, bytes);
            let target = GlobalLock(memory);
            if !target.is_null() {
                copy_nonoverlapping(text.as_ptr().cast::<u8>(), target.cast(), bytes);
                GlobalUnlock(memory);
                ok = !SetClipboardData(CF_UNICODETEXT, memory).is_null();
            }
        }
        CloseClipboard();
        ok
    }
}

pub fn get_clipboard() -> Option<String> {
    // SAFETY: as above; the clipboard owns the data, which is only read while
    // locked and before the clipboard closes.
    unsafe {
        if OpenClipboard(null_mut()) == 0 {
            return None;
        }
        let mut text = None;
        let memory = GetClipboardData(CF_UNICODETEXT);
        if !memory.is_null() {
            let data = GlobalLock(memory).cast::<u16>();
            if !data.is_null() {
                let mut len = 0;
                while *data.add(len) != 0 {
                    len += 1;
                }
                text = Some(String::from_utf16_lossy(std::slice::from_raw_parts(
                    data, len,
                )));
                GlobalUnlock(memory);
            }
        }
        CloseClipboard();
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_holder_of_a_lock_is_refused() {
        let name = format!("Local\\aeon-test-{}", std::process::id());
        assert!(single_instance(&name));
        assert!(!single_instance(&name));
    }
}
