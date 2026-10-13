//! Tray icon (logo drawn at the exact small-icon size) and its menu.
//!
//! The menu's top item is owner-drawn: the version, plus a green superscript
//! "↑<new version>" when an update is available (native menu theme for the
//! background and hover).
use tiny_skia::{FillRule, LineCap, Pixmap, PathBuilder, Rect, Transform};
use std::cell::Cell;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetSystemMetricsForDpi, SystemParametersInfoForDpi, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::draw::{self, col};

pub const ID_TOGGLE: u32 = 1;
pub const ID_TRACK: u32 = 3;
pub const ID_VERSION: u32 = 5;
pub const ID_SIZE_1: u32 = 10;
pub const ID_SIZE_15: u32 = 11;
pub const ID_SIZE_2: u32 = 12;
pub const ID_DISPLAY: u32 = 20; // + index
pub const ID_QUIT: u32 = 99;

pub struct MenuState {
    pub visible: bool,
    pub track: bool,
    pub scale: f32,
    pub display: usize,
    pub displays: usize,
}

/// CPU chip fused with the FPS sweep (line-icon style, like the pills).
fn logo(size: u32) -> Pixmap {
    let mut pix = Pixmap::new(size, size).unwrap();
    let k = size as f32 / 64.0;
    let ts = Transform::from_scale(k, k);
    let (g, y, r) = (col(0x4ade80), col(0xfacc15), col(0xef4444));
    let mut p = pix.as_mut();
    if let Some(bg) = draw::rrect(1.0, 1.0, 62.0, 62.0, 12.0) {
        p.fill_path(&bg, &draw::paint(col(0x171a21)), FillRule::Winding, ts, None);
        p.stroke_path(&bg, &draw::paint(col(0x39404e)), &draw::stroke(2.0, LineCap::Butt), ts, None);
    }
    let st = draw::stroke(4.0, LineCap::Butt);
    if let Some(rc) = Rect::from_ltrb(12.0, 12.0, 52.0, 52.0) {
        p.stroke_path(&PathBuilder::from_rect(rc), &draw::paint(g), &st, ts, None);
    }
    for v in [20.0, 32.0, 44.0] {
        for (a, b) in [((v, 10.0), (v, 4.0)), ((v, 54.0), (v, 60.0)), ((10.0, v), (4.0, v)), ((54.0, v), (60.0, v))] {
            if let Some(l) = draw::polyline(&[a, b], false) {
                p.stroke_path(&l, &draw::paint(g), &st, ts, None);
            }
        }
    }
    // PIL arcs go clockwise in screen space: 135 -> 405 is a 270° sweep over the top.
    if let Some(a) = draw::arc_path(32.0, 32.0, 6.0, 6.0, -135.0, -270.0) {
        p.stroke_path(&a, &draw::paint(y), &st, ts, None);
    }
    if let Some(a) = draw::arc_path(32.0, 32.0, 6.0, 6.0, 60.0, -60.0) {
        p.stroke_path(&a, &draw::paint(r), &st, ts, None);
    }
    if let Some(l) = draw::polyline(&[(32.0, 32.0), (37.0, 27.0)], false) {
        p.stroke_path(&l, &draw::paint(y), &st, ts, None);
    }
    if let Some(c) = PathBuilder::from_circle(32.0, 32.0, 3.0) {
        p.fill_path(&c, &draw::paint(g), FillRule::Winding, ts, None);
    }
    pix
}

fn make_icon(size: u32) -> HICON {
    let pix = logo(size);
    unsafe {
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(color) = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            return HICON::default();
        };
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, (size * size * 4) as usize);
        for (d, px) in dst.as_chunks_mut::<4>().0.iter_mut().zip(pix.pixels()) {
            let c = px.demultiply();
            d.copy_from_slice(&[c.blue(), c.green(), c.red(), c.alpha()]);
        }
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, None);
        let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

fn data(hwnd: HWND, msg: u32, dpi: u32) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: msg,
        ..Default::default()
    };
    let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) }.max(16) as u32;
    nid.hIcon = make_icon(size);
    set_tip_text(&mut nid);
    nid
}

fn set_tip_text(nid: &mut NOTIFYICONDATAW) {
    nid.szTip = [0; 128];
    for (d, s) in nid.szTip.iter_mut().take(127).zip(crate::update::tip().encode_utf16()) {
        *d = s;
    }
}

/// Refresh the tooltip (update found / gone). Callable from any thread.
pub fn set_tip(hwnd: HWND) {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_TIP | NIF_SHOWTIP,
        ..Default::default()
    };
    set_tip_text(&mut nid);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

pub fn add(hwnd: HWND, msg: u32, dpi: u32) -> bool {
    let mut nid = data(hwnd, msg, dpi);
    let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &nid) }.as_bool();
    // Version 4: a left click arrives as NIN_SELECT and a right click as
    // WM_CONTEXTMENU, the same from the taskbar and the Win11 overflow flyout.
    nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    unsafe {
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
        let _ = DestroyIcon(nid.hIcon);
    }
    ok
}

pub fn remove(hwnd: HWND) {
    let nid = NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: 1, ..Default::default() };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Show the menu at the cursor; returns the chosen command id (0 = none).
pub fn menu(hwnd: HWND, st: &MenuState) -> u32 {
    unsafe {
        let Ok(m) = CreatePopupMenu() else { return 0 };
        let Ok(size) = CreatePopupMenu() else { return 0 };
        let item = |menu: HMENU, id: u32, text: &str, checked: bool| {
            let w = wide(text);
            let flags = MF_STRING | if checked { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(menu, flags, id as usize, PCWSTR(w.as_ptr()));
        };
        let _ = AppendMenuW(m, MF_OWNERDRAW, ID_VERSION as usize, PCWSTR::null());
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
        item(m, ID_TOGGLE, if st.visible { "Hide" } else { "Show" }, false);
        item(m, ID_TRACK, "Track", st.track);
        item(size, ID_SIZE_1, "1x", st.scale == 1.0);
        item(size, ID_SIZE_15, "1.5x", st.scale == 1.5);
        item(size, ID_SIZE_2, "2x", st.scale == 2.0);
        let w = wide("Size");
        let _ = AppendMenuW(m, MF_POPUP, size.0 as usize, PCWSTR(w.as_ptr()));
        if st.displays > 1 {
            if let Ok(disp) = CreatePopupMenu() {
                for i in 0..st.displays.min(16) {
                    let label = if i == 0 { "1 (primary)".to_string() } else { format!("{}", i + 1) };
                    item(disp, ID_DISPLAY + i as u32, &label, st.display == i);
                }
                let w = wide("Display");
                let _ = AppendMenuW(m, MF_POPUP, disp.0 as usize, PCWSTR(w.as_ptr()));
            }
        }
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
        item(m, ID_QUIT, "Quit", false);
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST), MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        MENU_DPI.set(dx.max(96));
        let _ = SetForegroundWindow(hwnd); // required so the menu closes on outside click
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, Default::default(), Default::default());
        let _ = DestroyMenu(m);
        cmd.0 as u32
    }
}

thread_local! {
    /// DPI of the monitor the menu opens on (for the owner-drawn item).
    static MENU_DPI: Cell<u32> = const { Cell::new(96) };
}

pub fn menu_dpi() -> u32 {
    MENU_DPI.get()
}

/// Version item text: ("Version x", Some("↑y") when an update is available).
pub fn version_label() -> (String, Option<String>) {
    (format!("Version {}", crate::update::VERSION), crate::update::latest().map(|v| format!("\u{2191}{v}")))
}

const GREEN: COLORREF = COLORREF(0x004a_a316); // #16a34a (0x00BBGGRR)

/// Menu font at `dpi`, and a smaller semibold one for the superscript.
unsafe fn fonts(dpi: u32) -> (HFONT, HFONT) {
    let mut ncm = NONCLIENTMETRICSW { cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
    let _ = SystemParametersInfoForDpi(SPI_GETNONCLIENTMETRICS.0, ncm.cbSize, Some(&mut ncm as *mut _ as _), 0, dpi);
    let mut small = ncm.lfMenuFont;
    small.lfHeight = small.lfHeight * 3 / 4;
    small.lfWeight = 600;
    (CreateFontIndirectW(&ncm.lfMenuFont), CreateFontIndirectW(&small))
}

struct Layout {
    main: Vec<u16>,
    sup: Vec<u16>,
    w_main: i32,
    w_sup: i32,
    gap: i32,
    pad: i32,
    tm_main: TEXTMETRICW,
    tm_sup: TEXTMETRICW,
}

unsafe fn layout(hdc: HDC, big: HFONT, small: HFONT, dpi: u32, label: &(String, Option<String>)) -> Layout {
    let main: Vec<u16> = label.0.encode_utf16().collect();
    let sup: Vec<u16> = label.1.as_deref().unwrap_or("").encode_utf16().collect();
    let (mut sz_main, mut sz_sup) = (SIZE::default(), SIZE::default());
    let (mut tm_main, mut tm_sup) = (TEXTMETRICW::default(), TEXTMETRICW::default());
    let old = SelectObject(hdc, big.into());
    let _ = GetTextExtentPoint32W(hdc, &main, &mut sz_main);
    let _ = GetTextMetricsW(hdc, &mut tm_main);
    SelectObject(hdc, small.into());
    if !sup.is_empty() {
        let _ = GetTextExtentPoint32W(hdc, &sup, &mut sz_sup);
    }
    let _ = GetTextMetricsW(hdc, &mut tm_sup);
    SelectObject(hdc, old);
    let k = |v: i32| v * dpi as i32 / 96;
    let gap = if sup.is_empty() { 0 } else { k(3) };
    Layout { main, sup, w_main: sz_main.cx, w_sup: sz_sup.cx, gap, pad: k(6), tm_main, tm_sup }
}

/// WM_MEASUREITEM for the version item.
pub fn measure(hwnd: HWND, mi: &mut MEASUREITEMSTRUCT, label: &(String, Option<String>)) {
    let dpi = MENU_DPI.get();
    unsafe {
        let hdc = GetDC(Some(hwnd));
        let (big, small) = fonts(dpi);
        let l = layout(hdc, big, small, dpi, label);
        mi.itemWidth = (l.w_main + l.gap + l.w_sup + 2 * l.pad) as u32;
        mi.itemHeight = (l.tm_main.tmHeight + 2 * l.pad) as u32;
        let _ = DeleteObject(big.into());
        let _ = DeleteObject(small.into());
        ReleaseDC(Some(hwnd), hdc);
    }
}

/// WM_DRAWITEM for the version item: centered "Version x", then a green "↑y"
/// raised like an exponent.
pub fn draw(di: &DRAWITEMSTRUCT, label: &(String, Option<String>), dpi: u32) {
    let hdc = di.hDC;
    let rc = di.rcItem;
    let hot = di.itemState.0 & ODS_SELECTED.0 != 0;
    unsafe {
        let theme = OpenThemeData(None, w!("MENU"));
        let text = if !theme.is_invalid() {
            let _ = DrawThemeBackground(theme, hdc, MENU_POPUPBACKGROUND.0, 0, &rc, None);
            if hot {
                let _ = DrawThemeBackground(theme, hdc, MENU_POPUPITEM.0, MPI_HOT.0, &rc, None);
            }
            let c = GetThemeColor(theme, MENU_POPUPITEM.0, MPI_NORMAL.0, TMT_TEXTCOLOR).unwrap_or(COLORREF(GetSysColor(COLOR_MENUTEXT)));
            let _ = CloseThemeData(theme);
            c
        } else {
            FillRect(hdc, &rc, GetSysColorBrush(if hot { COLOR_HIGHLIGHT } else { COLOR_MENU }));
            COLORREF(GetSysColor(if hot { COLOR_HIGHLIGHTTEXT } else { COLOR_MENUTEXT }))
        };
        let (big, small) = fonts(dpi);
        let l = layout(hdc, big, small, dpi, label);
        let x = rc.left + (rc.right - rc.left - l.w_main - l.gap - l.w_sup) / 2;
        let y = rc.top + (rc.bottom - rc.top - l.tm_main.tmHeight) / 2;
        let _ = SetBkMode(hdc, TRANSPARENT);
        let old = SelectObject(hdc, big.into());
        SetTextColor(hdc, text);
        let _ = TextOutW(hdc, x, y, &l.main);
        if !l.sup.is_empty() {
            // Superscript: the small run's baseline sits 45% up the main text's ascent.
            let base = y + l.tm_main.tmAscent - l.tm_main.tmAscent * 45 / 100;
            SelectObject(hdc, small.into());
            SetTextColor(hdc, GREEN);
            let _ = TextOutW(hdc, x + l.w_main + l.gap, base - l.tm_sup.tmAscent, &l.sup);
        }
        SelectObject(hdc, old);
        let _ = DeleteObject(big.into());
        let _ = DeleteObject(small.into());
    }
}

/// Render the version item (normal, then hover) off-screen for the self-test:
/// (width, height, RGBA of both states stacked).
pub fn preview(label: &(String, Option<String>), dpi: u32) -> (u32, u32, Vec<u8>) {
    let k = |v: i32| v * dpi as i32 / 96;
    let (w, h) = (k(200), k(28));
    let mut out = Vec::new();
    unsafe {
        let dc = CreateCompatibleDC(None);
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -(2 * h),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(bmp) = CreateDIBSection(Some(dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(dc);
            return (0, 0, out);
        };
        let old = SelectObject(dc, bmp.into());
        for (i, state) in [ODS_FLAGS(0), ODS_SELECTED].into_iter().enumerate() {
            let di = DRAWITEMSTRUCT {
                CtlType: ODT_MENU,
                itemID: ID_VERSION,
                itemState: state,
                hDC: dc,
                rcItem: RECT { left: 0, top: i as i32 * h, right: w, bottom: (i as i32 + 1) * h },
                ..Default::default()
            };
            draw(&di, label, dpi);
        }
        let _ = GdiFlush();
        let px = std::slice::from_raw_parts(bits as *const u8, (w * 2 * h * 4) as usize);
        for c in px.as_chunks::<4>().0 {
            out.extend_from_slice(&[c[2], c[1], c[0], 255]);
        }
        SelectObject(dc, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(dc);
    }
    (w as u32, 2 * h as u32, out)
}

#[cfg(test)]
mod tests {
    /// Regenerate assets/icon.ico (exe + installer icon) from the tray logo:
    /// `cargo test export_icon -- --ignored`
    #[test]
    #[ignore]
    fn export_icon() {
        let sizes = [16u32, 20, 24, 32, 40, 48, 64, 256];
        let pngs: Vec<Vec<u8>> = sizes.iter().map(|&s| super::logo(s).encode_png().unwrap()).collect();
        let mut ico = vec![0u8, 0, 1, 0];
        ico.extend((sizes.len() as u16).to_le_bytes());
        let mut off = 6 + 16 * sizes.len() as u32;
        for (&s, png) in sizes.iter().zip(&pngs) {
            let b = if s >= 256 { 0 } else { s as u8 };
            ico.extend([b, b, 0, 0]);
            ico.extend(1u16.to_le_bytes());
            ico.extend(32u16.to_le_bytes());
            ico.extend((png.len() as u32).to_le_bytes());
            ico.extend(off.to_le_bytes());
            off += png.len() as u32;
        }
        for png in &pngs {
            ico.extend(png);
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("icon.ico"), ico).unwrap();
    }
}
