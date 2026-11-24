//! Win32 for the launcher: the global hotkey, the clipboard, the
//! one-resident-per-session lock, and the light/dark theme. The only `unsafe`
//! code in this crate.

use std::ffi::c_void;
use std::ptr::{copy_nonoverlapping, null, null_mut};
use std::sync::mpsc;

use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_SUCCESS, GetLastError, HANDLE, POINT, SIZE,
};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDIBits, GetObjectW, HBITMAP,
};
use windows_sys::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_QUERY_CONTENT_LENGTH,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect,
    WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
    WinHttpSendRequest,
};
use windows_sys::Win32::Security::Cryptography::{BCRYPT_SHA256_ALG_HANDLE, BCryptHash};
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_DWORD, RRF_RT_REG_DWORD, RegGetValueW, RegSetKeyValueW,
};
use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentThreadId};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
};
use windows_sys::Win32::UI::Shell::{
    FOLDERID_AppsFolder, KF_FLAG_DEFAULT, SHCreateItemInKnownFolder, SIIGBF_BIGGERSIZEOK,
    SIIGBF_ICONONLY, ShellExecuteW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetCursorPos, GetMessageW, HWND_BROADCAST, MSG, PostMessageW, PostThreadMessageW,
    RegisterWindowMessageW, SMTO_ABORTIFHUNG, SW_SHOWNORMAL, SendMessageTimeoutW, WM_HOTKEY,
    WM_QUIT, WM_SETTINGCHANGE,
};
use windows_sys::core::{GUID, HRESULT};

const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

/// `IShellItemImageFactory`: the shell's own icon for any item, Store apps
/// included. windows-sys has no COM wrappers, so its vtable is spelled out.
const IID_ISHELL_ITEM_IMAGE_FACTORY: GUID = GUID::from_u128(0xbcc18b79_ba16_442f_80c4_8a59c30c463b);

#[repr(C)]
struct ImageFactoryVtbl {
    query_interface: usize,
    add_ref: usize,
    release: unsafe extern "system" fn(this: *mut c_void) -> u32,
    get_image: unsafe extern "system" fn(
        this: *mut c_void,
        size: SIZE,
        flags: i32,
        bitmap: *mut HBITMAP,
    ) -> HRESULT,
}

/// Opens `url` in the default browser.
pub fn open_url(url: &str) -> bool {
    let (verb, url) = (wide("open"), wide(url));
    // SAFETY: NUL-terminated strings; ShellExecuteW returns a value above 32
    // on success.
    let result = unsafe {
        ShellExecuteW(
            null_mut(),
            verb.as_ptr(),
            url.as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        )
    };
    result as isize > 32
}

/// Prepares the calling thread for [`app_icon`]. Call once per thread.
pub fn init_com() {
    // SAFETY: plain initialization; a second call on the same thread only
    // returns S_FALSE.
    unsafe { CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32) };
}

/// The Start menu icon of an app, `size` pixels square, as the shell draws it.
/// Takes a few to tens of milliseconds: call it on a background thread that
/// called [`init_com`].
pub fn app_icon(id: &str, size: u32) -> Option<Icon> {
    let id = wide(id);
    let mut factory: *mut c_void = null_mut();
    // SAFETY: `id` is NUL-terminated and `factory` receives the requested
    // interface, released below.
    let hr = unsafe {
        SHCreateItemInKnownFolder(
            &FOLDERID_AppsFolder,
            KF_FLAG_DEFAULT as u32,
            id.as_ptr(),
            &IID_ISHELL_ITEM_IMAGE_FACTORY,
            &mut factory,
        )
    };
    if hr < 0 || factory.is_null() {
        return None;
    }
    // SAFETY: a COM object's first field points at its vtable, which for this
    // interface starts with the layout above.
    let vtbl = unsafe { &**(factory as *mut *const ImageFactoryVtbl) };
    let mut bitmap: HBITMAP = null_mut();
    let side = size as i32;
    // SAFETY: `factory` is live; `bitmap` receives an HBITMAP we delete.
    let hr = unsafe {
        (vtbl.get_image)(
            factory,
            SIZE { cx: side, cy: side },
            SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK,
            &mut bitmap,
        )
    };
    // SAFETY: balances the reference SHCreateItemInKnownFolder handed out.
    unsafe { (vtbl.release)(factory) };
    if hr < 0 || bitmap.is_null() {
        return None;
    }
    let icon = bitmap_pixels(bitmap);
    // SAFETY: the bitmap is ours and no longer used.
    unsafe { DeleteObject(bitmap) };
    icon
}

/// Reads a 32-bit shell bitmap as premultiplied `0xAARRGGBB`, top row first.
fn bitmap_pixels(bitmap: HBITMAP) -> Option<Icon> {
    // SAFETY: BITMAP and BITMAPINFO are plain data; GetObjectW and GetDIBits
    // write at most their sizes, and `pixels` holds width * height u32s.
    unsafe {
        let mut info: BITMAP = std::mem::zeroed();
        if GetObjectW(
            bitmap,
            size_of::<BITMAP>() as i32,
            (&mut info as *mut BITMAP).cast(),
        ) == 0
        {
            return None;
        }
        let (width, height) = (info.bmWidth, info.bmHeight.abs());
        if width <= 0 || height <= 0 || width > 256 || height > 256 {
            return None;
        }
        let mut header: BITMAPINFO = std::mem::zeroed();
        header.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        header.bmiHeader.biWidth = width;
        header.bmiHeader.biHeight = -height; // top-down rows
        header.bmiHeader.biPlanes = 1;
        header.bmiHeader.biBitCount = 32;
        header.bmiHeader.biCompression = BI_RGB;
        let mut pixels = vec![0u32; (width * height) as usize];
        let dc = CreateCompatibleDC(null_mut());
        let rows = GetDIBits(
            dc,
            bitmap,
            0,
            height as u32,
            pixels.as_mut_ptr().cast(),
            &mut header,
            DIB_RGB_COLORS,
        );
        DeleteDC(dc);
        (rows == height).then_some(Icon {
            width: width as u32,
            height: height as u32,
            pixels,
        })
    }
}

/// Whether apps use the light theme. `None` when Windows has no setting yet.
pub fn light_theme() -> Option<bool> {
    let (key, value) = (wide(PERSONALIZE), wide("AppsUseLightTheme"));
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: the strings are NUL-terminated and `data` holds the DWORD
    // RRF_RT_REG_DWORD restricts the read to.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut data as *mut u32).cast::<c_void>(),
            &mut size,
        )
    };
    (status == ERROR_SUCCESS).then_some(data != 0)
}

/// Sets both the apps' and the system's (taskbar, Start) theme, the way the
/// Settings app does, then tells every window so they repaint without a
/// sign-out. Blocks for up to a few hundred ms on unresponsive windows: call
/// it off the UI thread.
pub fn set_light_theme(light: bool) -> bool {
    let key = wide(PERSONALIZE);
    let data = u32::from(light);
    let mut ok = true;
    for name in ["AppsUseLightTheme", "SystemUsesLightTheme"] {
        let name = wide(name);
        // SAFETY: NUL-terminated strings and a 4-byte DWORD, as REG_DWORD needs.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_DWORD,
                (&data as *const u32).cast::<c_void>(),
                size_of::<u32>() as u32,
            )
        };
        ok &= status == ERROR_SUCCESS;
    }
    let area = wide("ImmersiveColorSet");
    // SAFETY: `area` outlives the call; SMTO_ABORTIFHUNG keeps a hung window
    // from stalling the broadcast past its timeout.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            area.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            200,
            null_mut(),
        );
    }
    ok
}

use crate::{Hotkey, Icon};

const CF_UNICODETEXT: u32 = 13;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Registers `hotkey` for the whole session and calls `on_press` from a
/// dedicated thread on every press. Fails when another app already owns it.
///
/// The hotkey belongs to the thread that registered it and arrives as a
/// thread message, which winit's loop would drop, so it gets its own loop.
pub fn listen(
    hotkey: Hotkey,
    on_press: impl Fn() + Send + 'static,
) -> Result<HotkeyListener, String> {
    let (registered, result) = mpsc::channel();
    std::thread::Builder::new()
        .name("hotkey".into())
        .spawn(move || {
            // SAFETY: a null window binds the hotkey to this thread's queue.
            let ok = unsafe {
                RegisterHotKey(null_mut(), 1, hotkey.modifiers | MOD_NOREPEAT, hotkey.key)
            };
            // SAFETY: no preconditions.
            let thread = unsafe { GetCurrentThreadId() };
            let _ = registered.send((ok != 0).then_some(thread));
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
            // SAFETY: releases the registration this thread made.
            unsafe { UnregisterHotKey(null_mut(), 1) };
        })
        .map_err(|e| e.to_string())?;
    match result.recv() {
        Ok(Some(thread)) => Ok(HotkeyListener { thread }),
        _ => Err(format!("{hotkey} is already used by another app")),
    }
}

/// A registered hotkey. Dropping it ends its thread and frees the keys, so a
/// changed hotkey can be registered while the resident keeps running.
pub struct HotkeyListener {
    thread: u32,
}

impl Drop for HotkeyListener {
    fn drop(&mut self) {
        // SAFETY: WM_QUIT ends the listener's GetMessageW loop.
        unsafe { PostThreadMessageW(self.thread, WM_QUIT, 0, 0) };
    }
}

/// The resident's hidden tray window, which terminals find by class.
/// Development builds use their own, so a debug terminal talks to a debug
/// resident and never to an installed one.
const TRAY_CLASS: &str = if cfg!(debug_assertions) {
    "AeonTrayDev"
} else {
    "AeonTray"
};

/// The message a terminal sends the resident; the command travels in wParam.
const RESIDENT_COMMAND: &str = "AeonResidentCommand";

/// Sends `command` to the running resident. `false` when none runs.
pub fn send_to_resident(command: crate::ResidentCommand) -> bool {
    let (class, message) = (wide(TRAY_CLASS), wide(RESIDENT_COMMAND));
    // SAFETY: NUL-terminated strings; posting to a window that vanished in
    // between only fails.
    unsafe {
        let window = FindWindowW(class.as_ptr(), null());
        !window.is_null()
            && PostMessageW(
                window,
                RegisterWindowMessageW(message.as_ptr()),
                command as usize,
                0,
            ) != 0
    }
}

/// Tells a running resident to re-read config.toml.
pub fn notify_settings_changed() {
    send_to_resident(crate::ResidentCommand::SettingsChanged);
}

/// GETs an https `url`, following redirects, through the system proxy, and
/// returns the HTTP status. Only a 200's body is read: `on_chunk` receives
/// it piece by piece and `on_progress(done, total)` reports bytes, `total`
/// being the length when the server gives it. Blocks: call it on a
/// background thread.
pub fn http_get(
    url: &str,
    mut on_chunk: impl FnMut(&[u8]) -> Result<(), String>,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<u32, String> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| format!("not an https URL: {url}"))?;
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let path = if path.is_empty() { "/" } else { path };

    /// Closes a WinHTTP handle on every path out.
    struct Handle(*mut c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: a handle WinHTTP returned, closed once.
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }
    let fail = |what: &str| format!("{what} failed (error {})", std::io::Error::last_os_error());

    let agent = wide(&format!("Aeon/{}", env!("CARGO_PKG_VERSION")));
    let (host, path, get) = (wide(host), wide(path), wide("GET"));
    // SAFETY: every string is NUL-terminated and outlives the calls; handles
    // are checked before use and closed by `Handle`.
    unsafe {
        let session = Handle(WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            null(),
            null(),
            0,
        ));
        if session.0.is_null() {
            return Err(fail("WinHttpOpen"));
        }
        let connection = Handle(WinHttpConnect(session.0, host.as_ptr(), 443, 0));
        if connection.0.is_null() {
            return Err(fail("connecting"));
        }
        let request = Handle(WinHttpOpenRequest(
            connection.0,
            get.as_ptr(),
            path.as_ptr(),
            null(),
            null(),
            null(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            return Err(fail("opening the request"));
        }
        if WinHttpSendRequest(request.0, null(), 0, null(), 0, 0, 0) == 0
            || WinHttpReceiveResponse(request.0, null_mut()) == 0
        {
            return Err(fail("the request"));
        }
        let number = |info: u32| {
            let mut value = 0u32;
            let mut size = size_of::<u32>() as u32;
            let ok = WinHttpQueryHeaders(
                request.0,
                info | WINHTTP_QUERY_FLAG_NUMBER,
                null(),
                (&mut value as *mut u32).cast(),
                &mut size,
                null_mut(),
            );
            (ok != 0).then_some(value)
        };
        let status = number(WINHTTP_QUERY_STATUS_CODE).unwrap_or(0);
        if status != 200 {
            return Ok(status);
        }
        let total = number(WINHTTP_QUERY_CONTENT_LENGTH).map(u64::from);
        let mut buf = vec![0u8; 64 * 1024];
        let mut done = 0u64;
        loop {
            let mut read = 0u32;
            if WinHttpReadData(
                request.0,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut read,
            ) == 0
            {
                return Err(fail("reading"));
            }
            if read == 0 {
                return Ok(status);
            }
            on_chunk(&buf[..read as usize])?;
            done += u64::from(read);
            on_progress(done, total);
        }
    }
}

/// Lowercase hex SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> Result<String, String> {
    let mut digest = [0u8; 32];
    // SAFETY: the pseudo-handle needs no setup; input and output buffers are
    // valid for the lengths given.
    let status = unsafe {
        BCryptHash(
            BCRYPT_SHA256_ALG_HANDLE,
            null(),
            0,
            data.as_ptr(),
            data.len() as u32,
            digest.as_mut_ptr(),
            digest.len() as u32,
        )
    };
    if status < 0 {
        return Err(format!("hashing failed (status {status:#x})"));
    }
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
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
    fn a_dropped_hotkey_can_be_registered_again() {
        // An unusual combination, so the test never fights a real app for it.
        let hotkey: Hotkey = "Ctrl+Alt+Shift+F19".parse().unwrap();
        let first = listen(hotkey, || {}).expect("first registration");
        assert!(listen(hotkey, || {}).is_err(), "a hotkey has one owner");
        drop(first);
        // The listener thread unregisters asynchronously after WM_QUIT.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match listen(hotkey, || {}) {
                Ok(_) => break,
                Err(e) if std::time::Instant::now() > deadline => panic!("{e}"),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        }
    }

    #[test]
    fn sha256_matches_the_standard_vector() {
        assert_eq!(
            sha256(b"abc").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn second_holder_of_a_lock_is_refused() {
        let name = format!("Local\\aeon-test-{}", std::process::id());
        assert!(single_instance(&name));
        assert!(!single_instance(&name));
    }
}

/// The resident's icon in the notification area ("hidden icons"), with its
/// click and menu, on a thread of its own: like the hotkey, its messages go
/// to a window winit does not know about.
mod tray {
    use std::cell::RefCell;
    use std::ptr::{null, null_mut};
    use std::sync::mpsc;

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Shell::{
        NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
        NOTIFYICONDATAW, Shell_NotifyIconW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
        DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, HICON, IMAGE_ICON,
        LR_DEFAULTCOLOR, LoadImageW, MF_SEPARATOR, MF_STRING, MSG, PostMessageW, RegisterClassW,
        RegisterWindowMessageW, SM_CXSMICON, SM_CYSMICON, SetForegroundWindow, TPM_RETURNCMD,
        TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WM_APP, WM_LBUTTONUP, WM_NULL,
        WM_RBUTTONUP, WNDCLASSW,
    };

    use super::wide;
    use crate::TrayCommand;

    const CALLBACK: u32 = WM_APP + 1;
    const ICON_ID: u32 = 1;

    /// What the tray window procedure needs; it runs on the tray thread only.
    struct State {
        on_command: Box<dyn Fn(TrayCommand)>,
        /// Explorer broadcasts this after restarting; the icon must be re-added.
        taskbar_created: u32,
        /// Commands from terminals; see `send_to_resident`.
        command_message: u32,
        hwnd: HWND,
        icon: HICON,
        tooltip: String,
        launcher_label: String,
    }

    thread_local! {
        static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    }

    /// Removes the icon when dropped, so it does not linger after Aeon exits.
    pub struct Tray {
        hwnd: usize,
    }

    impl Tray {
        /// Shows a notification from the icon, such as "Update ready".
        pub fn notify(&self, title: &str, text: &str) {
            let mut data = notify_data(self.hwnd as HWND, null_mut(), "");
            data.uFlags = NIF_INFO;
            data.dwInfoFlags = NIIF_INFO;
            for (slot, unit) in data
                .szInfoTitle
                .iter_mut()
                .zip(title.encode_utf16().take(63))
            {
                *slot = unit;
            }
            for (slot, unit) in data.szInfo.iter_mut().zip(text.encode_utf16().take(255)) {
                *slot = unit;
            }
            // SAFETY: modifies our own icon by window and id.
            unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
        }
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            let data = notify_data(self.hwnd as HWND, null_mut(), "");
            // SAFETY: deleting by window and id; harmless if already gone.
            unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
        }
    }

    fn notify_data(hwnd: HWND, icon: HICON, tooltip: &str) -> NOTIFYICONDATAW {
        // SAFETY: NOTIFYICONDATAW is plain data; all-zero is its empty state.
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = ICON_ID;
        data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        data.uCallbackMessage = CALLBACK;
        data.hIcon = icon;
        for (slot, unit) in data.szTip.iter_mut().zip(tooltip.encode_utf16().take(127)) {
            *slot = unit;
        }
        data
    }

    fn add_icon(state: &State) -> bool {
        let data = notify_data(state.hwnd, state.icon, &state.tooltip);
        // SAFETY: `data` is a complete NOTIFYICONDATAW for our window.
        unsafe { Shell_NotifyIconW(NIM_ADD, &data) != 0 }
    }

    /// Shows the icon and calls `on_command` from the tray thread when the
    /// user clicks it (`OpenLauncher`) or picks a menu item. `launcher_label`
    /// names the menu's first item, hotkey included.
    pub fn show(
        tooltip: &str,
        launcher_label: &str,
        on_command: impl Fn(TrayCommand) + Send + 'static,
    ) -> Result<Tray, String> {
        let (ready, result) = mpsc::channel();
        let (tooltip, launcher_label) = (tooltip.to_owned(), launcher_label.to_owned());
        std::thread::Builder::new()
            .name("tray".into())
            .spawn(move || {
                let class = wide(super::TRAY_CLASS);
                let title = wide("Aeon");
                // SAFETY: a plain window class and a hidden top-level window
                // owned by this thread; the icon is the exe's resource 1.
                let (hwnd, icon) = unsafe {
                    let instance = GetModuleHandleW(null());
                    // SAFETY (zeroed): WNDCLASSW is plain data.
                    let mut wc: WNDCLASSW = std::mem::zeroed();
                    wc.lpfnWndProc = Some(window_proc);
                    wc.hInstance = instance;
                    wc.lpszClassName = class.as_ptr();
                    RegisterClassW(&wc);
                    let hwnd = CreateWindowExW(
                        0,
                        class.as_ptr(),
                        title.as_ptr(),
                        0,
                        0,
                        0,
                        0,
                        0,
                        null_mut(),
                        null_mut(),
                        instance,
                        null(),
                    );
                    let icon = LoadImageW(
                        instance,
                        // MAKEINTRESOURCE(1): resource ids travel as pointer values.
                        std::ptr::without_provenance::<u16>(1),
                        IMAGE_ICON,
                        GetSystemMetrics(SM_CXSMICON),
                        GetSystemMetrics(SM_CYSMICON),
                        LR_DEFAULTCOLOR,
                    ) as HICON;
                    (hwnd, icon)
                };
                if hwnd.is_null() {
                    let _ = ready.send(Err("could not create the tray window".to_owned()));
                    return;
                }
                let taskbar_created = {
                    let name = wide("TaskbarCreated");
                    // SAFETY: NUL-terminated name.
                    unsafe { RegisterWindowMessageW(name.as_ptr()) }
                };
                let state = State {
                    on_command: Box::new(on_command),
                    taskbar_created,
                    command_message: {
                        let name = wide(super::RESIDENT_COMMAND);
                        // SAFETY: NUL-terminated name.
                        unsafe { RegisterWindowMessageW(name.as_ptr()) }
                    },
                    hwnd,
                    icon,
                    tooltip,
                    launcher_label,
                };
                let added = add_icon(&state);
                STATE.with(|s| *s.borrow_mut() = Some(state));
                let _ = ready.send(if added {
                    Ok(hwnd as usize)
                } else {
                    Err("the notification area refused the icon".to_owned())
                });
                // SAFETY: MSG is plain data; the loop pumps this thread's queue.
                let mut msg: MSG = unsafe { std::mem::zeroed() };
                // SAFETY: `msg` is valid for writes; a null window reads every
                // message for this thread's windows.
                while unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) } > 0 {
                    // SAFETY: `msg` came from GetMessageW.
                    unsafe {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let hwnd = result
            .recv()
            .map_err(|_| "the tray thread stopped".to_owned())??;
        Ok(Tray { hwnd })
    }

    const MENU_LAUNCHER: usize = 1;
    const MENU_TERMINAL: usize = 2;
    const MENU_SETTINGS: usize = 3;
    const MENU_QUIT: usize = 4;

    /// Shows the context menu at the mouse and returns the picked command.
    fn menu(hwnd: HWND, launcher_label: &str) -> Option<TrayCommand> {
        let items = [
            (MENU_LAUNCHER, launcher_label),
            (MENU_TERMINAL, "New terminal"),
            (MENU_SETTINGS, "Open settings"),
            (0, ""),
            (MENU_QUIT, "Quit Aeon"),
        ];
        // SAFETY: the menu is created, shown and destroyed here; the strings
        // outlive AppendMenuW, which copies them.
        let picked = unsafe {
            let menu = CreatePopupMenu();
            for (id, label) in items {
                if id == 0 {
                    AppendMenuW(menu, MF_SEPARATOR, 0, null());
                } else {
                    let label = wide(label);
                    AppendMenuW(menu, MF_STRING, id, label.as_ptr());
                }
            }
            let mut point = POINT { x: 0, y: 0 };
            GetCursorPos(&mut point);
            // Without this the menu stays open when the user clicks elsewhere.
            SetForegroundWindow(hwnd);
            let picked = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                0,
                hwnd,
                null(),
            ) as usize;
            PostMessageW(hwnd, WM_NULL, 0, 0);
            DestroyMenu(menu);
            picked
        };
        match picked {
            MENU_LAUNCHER => Some(TrayCommand::OpenLauncher),
            MENU_TERMINAL => Some(TrayCommand::NewTerminal),
            MENU_SETTINGS => Some(TrayCommand::OpenSettings),
            MENU_QUIT => Some(TrayCommand::Quit),
            _ => None,
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let handled = STATE.with(|cell| {
            let state = cell.borrow();
            let state = state.as_ref()?;
            if msg == CALLBACK {
                let command = match lparam as u32 {
                    WM_LBUTTONUP => Some(TrayCommand::OpenLauncher),
                    WM_RBUTTONUP => menu(hwnd, &state.launcher_label),
                    _ => None,
                };
                if let Some(command) = command {
                    (state.on_command)(command);
                }
                return Some(0);
            }
            if msg == state.taskbar_created {
                add_icon(state);
                return Some(0);
            }
            if msg == state.command_message {
                if let Some(command) = crate::ResidentCommand::from_code(wparam) {
                    (state.on_command)(command.tray_command());
                }
                return Some(0);
            }
            None
        });
        // SAFETY: default handling for everything this procedure ignores.
        handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) })
    }
}

pub use tray::{Tray, show as show_tray};
