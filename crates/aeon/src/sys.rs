//! Interface text through GDI: Windows rasterizes Segoe UI with ClearType from
//! the system's own font cache. Loading the same faces with fontdue costs 9 to
//! 40 MB, because it builds every glyph up front.

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::SIZE;
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, GetTextExtentExPointW, GetTextExtentPoint32W,
    GetTextMetricsW, HBITMAP, HDC, HFONT, OUT_TT_PRECIS, SelectObject, SetBkMode, SetTextColor,
    TEXTMETRICW, TRANSPARENT, TextOutW,
};

/// Which family a run is set in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    /// Segoe UI Variable on Windows 11, Segoe UI before it.
    Ui,
    /// Cascadia Mono, for monograms and anything code.
    Code,
    /// Segoe Fluent Icons on Windows 11, Segoe MDL2 Assets before it.
    Icons,
    /// The terminal's fallbacks for glyphs its font lacks, by index into
    /// [`FALLBACK_FACES`].
    Fallback(u8),
}

/// Where the terminal looks for a glyph its font lacks, in order: symbols,
/// the Nerd Fonts prompt themes and Terminal-Icons draw from (when
/// installed), then emoji, as Windows draws them in one colour.
pub const FALLBACK_FACES: &[&str] = &[
    "Segoe UI Symbol",
    "Symbols Nerd Font Mono",
    "CaskaydiaCove Nerd Font Mono",
    "FiraCode Nerd Font Mono",
    "JetBrainsMono Nerd Font Mono",
    "Hack Nerd Font Mono",
    "MesloLGS NF",
    "Segoe UI Emoji",
];

/// A rendered line of text: per-channel ClearType coverage, so it can be
/// blended in any colour over any background.
pub struct Run {
    pub width: u32,
    pub height: u32,
    /// `0x00RRGGBB` coverage per pixel, row-major.
    pub coverage: Vec<u32>,
}

pub struct TextRenderer {
    dc: HDC,
    bitmap: HBITMAP,
    bits: *mut u32,
    stride: u32,
    rows: u32,
    fonts: HashMap<(Face, u32, u16), HFONT>,
    ui_face: Vec<u16>,
    icon_face: Vec<u16>,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

impl TextRenderer {
    pub fn new() -> Option<Self> {
        let fonts = std::env::var_os("WINDIR")
            .map_or_else(|| "C:\\Windows".into(), std::path::PathBuf::from)
            .join("Fonts");
        let ui_face = if fonts.join("SegUIVar.ttf").exists() {
            "Segoe UI Variable Text"
        } else {
            "Segoe UI"
        };
        let icon_face = if fonts.join("SegoeIcons.ttf").exists() {
            "Segoe Fluent Icons"
        } else {
            "Segoe MDL2 Assets"
        };
        // SAFETY: a memory DC compatible with the screen; checked for null.
        let dc = unsafe { CreateCompatibleDC(null_mut()) };
        if dc.is_null() {
            return None;
        }
        // SAFETY: `dc` is valid; white text on a transparent background.
        unsafe {
            SetTextColor(dc, 0x00ff_ffff);
            SetBkMode(dc, TRANSPARENT as i32);
        }
        Some(Self {
            dc,
            bitmap: null_mut(),
            bits: null_mut(),
            stride: 0,
            rows: 0,
            fonts: HashMap::new(),
            ui_face: wide(ui_face),
            icon_face: wide(icon_face),
        })
    }

    /// Selects the font for `face` at `px` pixels and `weight` (400 regular,
    /// 600 semibold) into the DC.
    fn select(&mut self, face: Face, px: u32, weight: u16) {
        let font = *self.fonts.entry((face, px, weight)).or_insert_with(|| {
            let code = wide("Cascadia Mono");
            let fallback = match face {
                Face::Fallback(i) => {
                    wide(FALLBACK_FACES.get(usize::from(i)).copied().unwrap_or(""))
                }
                _ => Vec::new(),
            };
            let name = match face {
                Face::Ui => &self.ui_face,
                Face::Code => &code,
                Face::Icons => &self.icon_face,
                Face::Fallback(_) => &fallback,
            };
            // SAFETY: `name` is NUL-terminated and outlives the call. A negative
            // height asks for the em size rather than the cell height.
            unsafe {
                CreateFontW(
                    -(px as i32),
                    0,
                    0,
                    0,
                    i32::from(weight),
                    0,
                    0,
                    0,
                    u32::from(DEFAULT_CHARSET),
                    u32::from(OUT_TT_PRECIS),
                    u32::from(CLIP_DEFAULT_PRECIS),
                    u32::from(CLEARTYPE_QUALITY),
                    u32::from(DEFAULT_PITCH),
                    name.as_ptr(),
                )
            }
        });
        // SAFETY: both handles are valid; the previous font needs no restoring
        // because every font this DC sees is owned by `fonts`.
        unsafe { SelectObject(self.dc, font) };
    }

    /// How far the baseline is below the top of what `render` draws.
    pub fn ascent(&mut self, face: Face, px: u32, weight: u16) -> u32 {
        self.select(face, px, weight);
        // SAFETY: TEXTMETRICW is plain data that GetTextMetricsW fills.
        let mut metrics: TEXTMETRICW = unsafe { std::mem::zeroed() };
        // SAFETY: `metrics` is valid for writes; the DC has a font selected.
        unsafe { GetTextMetricsW(self.dc, &mut metrics) };
        metrics.tmAscent.max(0) as u32
    }

    /// Width and line height of `text`.
    pub fn measure(&mut self, text: &str, face: Face, px: u32, weight: u16) -> (u32, u32) {
        self.select(face, px, weight);
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut size = SIZE { cx: 0, cy: 0 };
        // SAFETY: `units` holds exactly the length passed.
        unsafe { GetTextExtentPoint32W(self.dc, units.as_ptr(), units.len() as i32, &mut size) };
        (size.cx.max(0) as u32, size.cy.max(0) as u32)
    }

    /// How many UTF-16 units of `text` fit in `max` pixels.
    pub fn fit(&mut self, text: &str, face: Face, px: u32, weight: u16, max: u32) -> usize {
        self.select(face, px, weight);
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut fit = 0;
        let mut size = SIZE { cx: 0, cy: 0 };
        // SAFETY: `units` holds exactly the length passed; no per-char widths.
        unsafe {
            GetTextExtentExPointW(
                self.dc,
                units.as_ptr(),
                units.len() as i32,
                max as i32,
                &mut fit,
                null_mut(),
                &mut size,
            )
        };
        fit.max(0) as usize
    }

    pub fn render(&mut self, text: &str, face: Face, px: u32, weight: u16) -> Run {
        let (width, height) = self.measure(text, face, px, weight);
        if width == 0 || height == 0 || !self.reserve(width, height) {
            return Run {
                width: 0,
                height: 0,
                coverage: Vec::new(),
            };
        }
        let units: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: the DIB holds at least `stride * rows` pixels, more than the
        // `width x height` corner cleared, drawn, and read here. GdiFlush makes
        // GDI's batched drawing visible in the bits before they are read.
        unsafe {
            for y in 0..height {
                let row = self.bits.add((y * self.stride) as usize);
                std::ptr::write_bytes(row, 0, width as usize);
            }
            TextOutW(self.dc, 0, 0, units.as_ptr(), units.len() as i32);
            GdiFlush();
            let mut coverage = Vec::with_capacity((width * height) as usize);
            for y in 0..height {
                let row = std::slice::from_raw_parts(
                    self.bits.add((y * self.stride) as usize),
                    width as usize,
                );
                coverage.extend(row.iter().map(|p| p & 0x00ff_ffff));
            }
            Run {
                width,
                height,
                coverage,
            }
        }
    }

    /// Grows the drawing surface to at least `width x height`.
    fn reserve(&mut self, width: u32, height: u32) -> bool {
        if width <= self.stride && height <= self.rows {
            return true;
        }
        let (w, h) = (
            width.max(self.stride).max(256),
            height.max(self.rows).max(64),
        );
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                // Negative: rows run top to bottom.
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = null_mut();
        // SAFETY: `info` describes a 32-bit top-down DIB; `bits` receives its
        // pixels, owned by the returned bitmap.
        let bitmap =
            unsafe { CreateDIBSection(self.dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0) };
        if bitmap.is_null() || bits.is_null() {
            return false;
        }
        // SAFETY: selecting the new bitmap releases the old one from the DC,
        // which can then be deleted.
        unsafe {
            SelectObject(self.dc, bitmap);
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap);
            }
        }
        self.bitmap = bitmap;
        self.bits = bits.cast();
        self.stride = w;
        self.rows = h;
        true
    }
}

impl Drop for TextRenderer {
    fn drop(&mut self) {
        // SAFETY: deleting the DC first releases the selected bitmap and font,
        // so each object is deleted exactly once and never while selected.
        unsafe {
            DeleteDC(self.dc);
            if !self.bitmap.is_null() {
                DeleteObject(self.bitmap);
            }
            for font in self.fonts.values() {
                DeleteObject(*font);
            }
        }
    }
}

/// Asks OK or Cancel in a standard warning dialog owned by `owner`. Returns
/// whether the user chose OK.
pub fn confirm(owner: isize, title: &str, text: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IDOK, MB_ICONWARNING, MB_OKCANCEL, MessageBoxW,
    };
    let (title, text) = (wide(title), wide(text));
    // SAFETY: both strings are NUL-terminated and outlive the call; `owner`
    // is this process's window or 0.
    unsafe {
        MessageBoxW(
            owner as _,
            text.as_ptr(),
            title.as_ptr(),
            MB_OKCANCEL | MB_ICONWARNING,
        ) == IDOK
    }
}

/// Paints the Windows title bar in the theme's colours, so it reads as part
/// of Aeon while keeping Snap layouts and the native buttons. Windows 10
/// ignores it.
pub fn tint_caption(hwnd: isize, pal: &crate::ui::Palette) {
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DwmSetWindowAttribute,
    };
    // COLORREF is 0x00BBGGRR.
    let colorref = |rgb: u32| ((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff);
    for (attribute, color) in [
        (DWMWA_CAPTION_COLOR, pal.canvas),
        (DWMWA_TEXT_COLOR, pal.muted),
        (DWMWA_BORDER_COLOR, pal.border),
    ] {
        let value = colorref(color);
        // SAFETY: `value` is a COLORREF that outlives the call, and its size
        // is what each of these attributes takes.
        unsafe {
            DwmSetWindowAttribute(
                hwnd as _,
                attribute as u32,
                (&value as *const u32).cast(),
                size_of::<u32>() as u32,
            );
        }
    }
}
