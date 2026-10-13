//! Tray icon (logo drawn at the exact small-icon size) and its menu.
use tiny_skia::{FillRule, LineCap, Pixmap, PathBuilder, Rect, Transform};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::draw::{self, col};

pub const ID_TOGGLE: u32 = 1;
pub const ID_TRACK: u32 = 3;
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
    for (d, s) in nid.szTip.iter_mut().zip("Resource Monitor".encode_utf16()) {
        *d = s;
    }
    nid
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
        let _ = SetForegroundWindow(hwnd); // required so the menu closes on outside click
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, Default::default(), Default::default());
        let _ = DestroyMenu(m);
        cmd.0 as u32
    }
}
