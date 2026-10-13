//! Windows, message loop, frame pacing and tray handling.
//!
//! Two layered, click-through, topmost popups with per-pixel alpha:
//! - main: capsules, text, gauges, ribbons; updated once per data tick
//! - snakes: a narrow transparent overlay over the icon column, redrawn at the
//!   monitor refresh rate while a snake moves (UpdateLayeredWindow cost scales
//!   with window size, so 60 Hz frames only push this small surface)
//!
//! Python used a chroma-key window plus Tk bindings that could never fire (the
//! window was click-through), and "hide to tray" had no way back; here all
//! controls live in the tray.
//! `WM_APP_CMD` runs a tray command id posted from another thread (the updater quits the app this way).
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::OnceLock;
use std::thread::Thread;
use std::time::Instant;

use tiny_skia::Pixmap;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODT_MENU};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::log::log;
use crate::settings::Settings;
use crate::tray;
use crate::ui::Ui;

const WM_APP_DATA: u32 = WM_APP + 1;
const WM_APP_TRAY: u32 = WM_APP + 2;
const WM_APP_FRAME: u32 = WM_APP + 3;
const WM_APP_CMD: u32 = WM_APP + 4;
/// From a second launch (Start Menu): show the running monitor.
const WM_APP_SHOW: u32 = WM_APP + 5;
const TIMER_TOPMOST: usize = 2;
const WINDOW_ALPHA: u8 = 237; // 0.93

/// A layered window plus its 32bpp DIB.
struct Layer {
    hwnd: HWND,
    dc: HDC,
    bmp: HBITMAP,
    bits: *mut u8,
    w: u32,
    h: u32,
}

impl Layer {
    fn new(hwnd: HWND) -> Self {
        Self { hwnd, dc: HDC::default(), bmp: HBITMAP::default(), bits: std::ptr::null_mut(), w: 0, h: 0 }
    }

    fn ensure(&mut self, w: u32, h: u32) -> bool {
        if self.w == w && self.h == h && !self.bits.is_null() {
            return true;
        }
        unsafe {
            if !self.bmp.is_invalid() {
                // The DC goes first: a bitmap still selected into a DC cannot be deleted.
                let _ = DeleteDC(self.dc);
                let _ = DeleteObject(self.bmp.into());
            }
            self.dc = CreateCompatibleDC(None);
            let bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let Ok(bmp) = CreateDIBSection(Some(self.dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0) else {
                self.bits = std::ptr::null_mut();
                return false;
            };
            SelectObject(self.dc, bmp.into());
            self.bmp = bmp;
            self.bits = bits as *mut u8;
        }
        self.w = w;
        self.h = h;
        true
    }

    /// Copy `pix` (premultiplied; RGBA, or BGRA when `bgra`) into the DIB and update the window.
    fn push(&mut self, pix: &Pixmap, at: POINT, bgra: bool) {
        if !self.ensure(pix.width(), pix.height()) {
            return;
        }
        let n = (self.w * self.h) as usize;
        let dst = unsafe { std::slice::from_raw_parts_mut(self.bits as *mut [u8; 4], n) };
        let src = pix.data().as_chunks::<4>().0;
        if bgra {
            dst.copy_from_slice(src);
        } else {
            for (d, p) in dst.iter_mut().zip(src) {
                *d = [p[2], p[1], p[0], p[3]];
            }
        }
        let size = SIZE { cx: self.w as i32, cy: self.h as i32 };
        let src = POINT::default();
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: WINDOW_ALPHA, AlphaFormat: AC_SRC_ALPHA as u8 };
        unsafe {
            let _ = UpdateLayeredWindow(self.hwnd, None, Some(&at), Some(&size), Some(self.dc), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA);
        }
    }
}

struct App {
    main: Layer,
    snakes: Layer,
    ui: Ui,
    settings: Settings,
    dpi: u32,
    /// Window top-left on the chosen display (10px in from its corner).
    origin: POINT,
    displays: usize,
    visible: bool,
    last_frame: Instant,
    anim_on: bool,
    taskbar_created: u32,
    /// Tray icon present; the only way to control the app, so retry until it is.
    tray_ok: bool,
}

/// Frame pacing: a thread waits for each DWM composition (= monitor refresh)
/// and posts one WM_APP_FRAME, only while a snake moves. At most one frame
/// message is queued at a time, so a slow frame never builds a backlog.
static ANIM_WANTED: AtomicBool = AtomicBool::new(false);
static FRAME_PENDING: AtomicBool = AtomicBool::new(false);
static PACER_HWND: AtomicIsize = AtomicIsize::new(0);
static PACER: OnceLock<Thread> = OnceLock::new();

fn pacer() {
    use windows::Win32::Graphics::Dwm::DwmFlush;
    loop {
        while !ANIM_WANTED.load(Ordering::Acquire) {
            std::thread::park();
        }
        // Blocks until the next composition pass. If DWM refuses or returns at
        // once (not composing: display off, some RDP states), pace at ~60 Hz.
        let t = Instant::now();
        let flushed = unsafe { DwmFlush() }.is_ok();
        let spent = t.elapsed();
        if !flushed || spent < std::time::Duration::from_millis(2) {
            std::thread::sleep(std::time::Duration::from_millis(16).saturating_sub(spent));
        }
        if ANIM_WANTED.load(Ordering::Acquire) && !FRAME_PENDING.swap(true, Ordering::AcqRel) {
            let h = HWND(PACER_HWND.load(Ordering::Acquire) as *mut _);
            unsafe {
                let _ = PostMessageW(Some(h), WM_APP_FRAME, WPARAM(0), LPARAM(0));
            }
        }
    }
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut a| a.as_mut().map(f)))
}

unsafe fn popup(class: PCWSTR, title: PCWSTR, hinst: HINSTANCE) -> Option<HWND> {
    let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
    CreateWindowExW(ex, class, title, WS_POPUP, 0, 0, 1, 1, None, None, Some(hinst), None).ok()
}

pub fn run() {
    unsafe {
        // Single instance (the Python version could be started twice). A
        // non-elevated launch cannot open the elevated instance's mutex
        // (access denied): that also means one is running.
        let mutex = CreateMutexW(None, true, w!("Local\\ResourceMonitorRs"));
        if mutex.is_err() || GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(h) = FindWindowW(w!("ResourceMonitorRs"), PCWSTR::null()) {
                let _ = PostMessageW(Some(h), WM_APP_SHOW, WPARAM(0), LPARAM(0));
            }
            return;
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        log(&format!("boot pid={} v{}", std::process::id(), crate::update::VERSION));
        crate::update::cleanup();

        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let main_class = w!("ResourceMonitorRs");
        let snake_class = w!("ResourceMonitorRsSnakes");
        for (class, proc_) in [(main_class, Some(wndproc as _)), (snake_class, Some(defproc as _))] {
            let wc = WNDCLASSW { lpfnWndProc: proc_, hInstance: hinst, lpszClassName: class, ..Default::default() };
            RegisterClassW(&wc);
        }
        let (Some(hwnd), Some(snake_hwnd)) = (popup(main_class, w!("Resource Monitor"), hinst), popup(snake_class, w!(""), hinst)) else {
            log("CreateWindowExW failed");
            return;
        };
        let settings = Settings::load();
        let (origin, dpi, displays) = placement(settings.display);
        let info = crate::metrics::info();
        let ui = Ui::new(info, settings.scale * dpi as f32 / 96.0, settings.track);
        let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));
        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                main: Layer::new(hwnd),
                snakes: Layer::new(snake_hwnd),
                ui,
                settings,
                dpi,
                origin,
                displays,
                visible: true,
                last_frame: Instant::now(),
                anim_on: false,
                taskbar_created,
                tray_ok: false,
            })
        });
        // Elevated (autostart task): let Explorer's restart broadcast through UIPI.
        let _ = ChangeWindowMessageFilterEx(hwnd, taskbar_created, MSGFLT_ALLOW, None);
        let _ = ChangeWindowMessageFilterEx(hwnd, WM_APP_SHOW, MSGFLT_ALLOW, None);
        let tray_ok = tray::add(hwnd, WM_APP_TRAY, dpi);
        if !tray_ok {
            log("tray add failed; retrying");
        }
        with_app(|a| a.tray_ok = tray_ok);
        PACER_HWND.store(hwnd.0 as isize, Ordering::Release);
        let _ = PACER.set(std::thread::Builder::new().name("pacer".into()).spawn(pacer).expect("pacer thread").thread().clone());
        with_app(|a| a.present());
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = ShowWindow(snake_hwnd, SW_SHOWNOACTIVATE);
        SetTimer(Some(hwnd), TIMER_TOPMOST, 2000, None);
        crate::metrics::start(hwnd, WM_APP_DATA);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        crate::metrics::stop();
        tray::remove(hwnd);
        log("quit");
    }
}

impl App {
    fn scale(&self) -> f32 {
        self.settings.scale * self.dpi as f32 / 96.0
    }

    /// Re-resolve the chosen display (monitors come and go) and its DPI.
    fn place(&mut self) {
        let (origin, dpi, displays) = placement(self.settings.display);
        self.origin = origin;
        self.displays = displays;
        if dpi != self.dpi {
            self.dpi = dpi;
            self.ui.set_scale(self.scale());
        }
        self.ui.invalidate();
    }

    /// Push both layers (after data, layout or settings changes).
    fn present(&mut self) {
        if !self.visible {
            return;
        }
        let at = self.origin;
        self.main.push(self.ui.base(), at, false);
        self.present_snakes();
    }

    /// Push only the snake overlay (every animation frame).
    fn present_snakes(&mut self) {
        if !self.visible {
            return;
        }
        let at = self.origin;
        self.snakes.push(self.ui.overlay(), at, true);
        self.sync_anim();
    }

    /// Run frame pacing only while a snake is actually moving.
    fn sync_anim(&mut self) {
        let want = self.visible && self.ui.animating();
        if want != self.anim_on {
            if want {
                self.last_frame = Instant::now();
            }
            ANIM_WANTED.store(want, Ordering::Release);
            if want {
                if let Some(t) = PACER.get() {
                    t.unpark();
                }
            }
            self.anim_on = want;
        }
    }

    fn animate(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.25);
        self.last_frame = now;
        self.ui.step(dt);
        self.present_snakes();
    }

    fn set_visible(&mut self, v: bool) {
        self.visible = v;
        let cmd = if v { SW_SHOWNOACTIVATE } else { SW_HIDE };
        if v {
            self.ui.invalidate();
            self.present();
        }
        unsafe {
            let _ = ShowWindow(self.main.hwnd, cmd);
            let _ = ShowWindow(self.snakes.hwnd, cmd);
        }
        self.sync_anim();
    }

    /// Keep both on top (other topmost windows can bury us), overlay above main.
    fn raise(&self) {
        unsafe {
            for h in [self.main.hwnd, self.snakes.hwnd] {
                let _ = SetWindowPos(h, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
                let ex = GetWindowLongW(h, GWL_EXSTYLE);
                if ex & WS_EX_TRANSPARENT.0 as i32 == 0 {
                    SetWindowLongW(h, GWL_EXSTYLE, ex | WS_EX_TRANSPARENT.0 as i32);
                }
            }
        }
    }

    fn command(&mut self, id: u32) {
        match id {
            tray::ID_TOGGLE => self.set_visible(!self.visible),
            tray::ID_TRACK => {
                self.settings.track = !self.settings.track;
                self.ui.track = self.settings.track;
                self.ui.invalidate();
            }
            tray::ID_SIZE_1 | tray::ID_SIZE_15 | tray::ID_SIZE_2 => {
                self.settings.scale = match id {
                    tray::ID_SIZE_1 => 1.0,
                    tray::ID_SIZE_15 => 1.5,
                    _ => 2.0,
                };
                self.ui.set_scale(self.scale());
                log(&format!("scale {}x", self.settings.scale));
            }
            id if (tray::ID_DISPLAY..tray::ID_DISPLAY + 16).contains(&id) => {
                self.settings.display = (id - tray::ID_DISPLAY) as usize;
                self.place();
                log(&format!("display {}", self.settings.display + 1));
            }
            tray::ID_VERSION => {
                crate::update::click(self.main.hwnd, WM_APP_CMD, tray::ID_QUIT);
                return;
            }
            tray::ID_QUIT => unsafe {
                let _ = DestroyWindow(self.snakes.hwnd);
                let _ = DestroyWindow(self.main.hwnd);
                return;
            },
            _ => return,
        }
        self.settings.save();
        self.present();
    }
}

unsafe extern "system" fn defproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    DefWindowProcW(hwnd, msg, wp, lp)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_APP_DATA => {
            if let Some(s) = crate::metrics::take_latest() {
                with_app(|a| {
                    a.ui.apply(&s);
                    a.present();
                });
            }
            LRESULT(0)
        }
        WM_APP_FRAME => {
            FRAME_PENDING.store(false, Ordering::Release);
            with_app(|a| a.animate());
            LRESULT(0)
        }
        WM_APP_SHOW => {
            with_app(|a| {
                if !a.visible {
                    a.set_visible(true);
                }
            });
            LRESULT(0)
        }
        WM_APP_CMD => {
            with_app(|a| a.command(wp.0 as u32));
            LRESULT(0)
        }
        WM_TIMER if wp.0 == TIMER_TOPMOST => {
            with_app(|a| {
                a.raise();
                crate::update::tick(a.main.hwnd);
                if !a.tray_ok {
                    // Shell not ready yet at logon: keep trying every 2s.
                    a.tray_ok = tray::add(a.main.hwnd, WM_APP_TRAY, a.dpi);
                }
            });
            LRESULT(0)
        }
        WM_APP_TRAY => {
            // NOTIFYICON_VERSION_4: the event is in LOWORD(lParam).
            // Right click opens the menu (Hide/Show lives there); left click does nothing.
            if (lp.0 & 0xffff) as u32 == WM_CONTEXTMENU {
                // No borrow is held while the modal menu loop runs (frames keep coming).
                let st = with_app(|a| tray::MenuState {
                    visible: a.visible,
                    track: a.settings.track,
                    scale: a.settings.scale,
                    display: if a.settings.display < a.displays { a.settings.display } else { 0 },
                    displays: a.displays,
                });
                if let Some(st) = st {
                    let id = tray::menu(hwnd, &st);
                    with_app(|a| a.command(id));
                }
            }
            LRESULT(0)
        }
        WM_MEASUREITEM => {
            let mi = &mut *(lp.0 as *mut MEASUREITEMSTRUCT);
            if mi.CtlType == ODT_MENU && mi.itemID == tray::ID_VERSION {
                tray::measure(hwnd, mi, &tray::version_label());
                return LRESULT(1);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_DRAWITEM => {
            let di = &*(lp.0 as *const DRAWITEMSTRUCT);
            if di.CtlType == ODT_MENU && di.itemID == tray::ID_VERSION {
                tray::draw(di, &tray::version_label(), tray::menu_dpi());
                return LRESULT(1);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_DPICHANGED | WM_DISPLAYCHANGE => {
            with_app(|a| {
                a.place();
                a.present();
            });
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => {
            let tc = with_app(|a| a.taskbar_created).unwrap_or(0);
            if tc != 0 && msg == tc {
                // Explorer restarted: the tray icon is gone, add it back.
                with_app(|a| a.tray_ok = tray::add(hwnd, WM_APP_TRAY, a.dpi));
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
    }
}

/// Monitors ordered primary first, then left-to-right: (rect, dpi, primary).
fn monitors() -> Vec<(RECT, u32, bool)> {
    unsafe extern "system" fn cb(h: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> windows::core::BOOL {
        let out = &mut *(lp.0 as *mut Vec<(RECT, u32, bool)>);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(h, &mut mi).as_bool() {
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            out.push((mi.rcMonitor, dx.max(96), mi.dwFlags & MONITORINFOF_PRIMARY != 0));
        }
        true.into()
    }
    let mut v: Vec<(RECT, u32, bool)> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    v.sort_by_key(|(r, _, primary)| (!primary, r.left, r.top));
    v
}

/// Window origin + DPI for display `index` (falls back to primary when it is gone).
fn placement(index: usize) -> (POINT, u32, usize) {
    let mons = monitors();
    let Some(&(r, dpi, _)) = mons.get(index).or(mons.first()) else {
        return (POINT { x: 10, y: 10 }, 96, 1);
    };
    let off = (10 * dpi / 96) as i32;
    (POINT { x: r.left + off, y: r.top + off }, dpi, mons.len())
}
