//! Game detection: fullscreen foreground app, else a known game exe running.
use std::time::{Duration, Instant};

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, RECT};
use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{
    EnumDisplaySettingsW, GetMonitorInfoW, MonitorFromWindow, DEVMODEW, ENUM_CURRENT_SETTINGS, MONITORINFOEXW,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, IsZoomed,
    GWL_STYLE, WS_CAPTION,
};

const GAME_EXES: &[&str] = &[
    "cs2.exe", "valorant.exe", "fortniteclient-win64-shipping.exe", "cod.exe", "cyberpunk2077.exe",
    "eldenring.exe", "minecraft.exe", "rdr2.exe", "apex_legends.exe", "overwatch.exe",
    "baldurs_gate3.exe", "b1.exe", "sekiro.exe", "dsr.exe", "witcher3.exe", "limbo.exe", "portal2.exe",
    "hl2.exe", "nightreign.exe", "darksoulsiii.exe", "darksoulsremastered.exe", "hollow_knight.exe",
    "terraria.exe", "stardewvalley.exe", "celeste.exe", "csgo.exe",
];

/// Fullscreen but not games: shell surfaces plus browsers / players / tools, so a
/// fullscreen YouTube video no longer flips the monitor into game mode.
const NOT_GAMES: &[&str] = &[
    "explorer.exe", "dwm.exe", "taskmgr.exe", "shellexperiencehost.exe", "startmenuexperiencehost.exe",
    "searchhost.exe", "textinputhost.exe", "lockapp.exe", "taskhostw.exe", "sihost.exe",
    "useroobebroker.exe", "snippingtool.exe", "snipandsketch.exe", "screenclippinghost.exe",
    "screencapturehost.exe", "applicationframehost.exe", "chrome.exe", "msedge.exe", "firefox.exe",
    "brave.exe", "opera.exe", "vivaldi.exe", "arc.exe", "vlc.exe", "mpv.exe", "mpc-hc.exe", "mpc-hc64.exe",
    "mpc-be64.exe", "potplayermini64.exe", "wmplayer.exe", "microsoft.media.player.exe", "video.ui.exe",
    "powerpnt.exe", "windowsterminal.exe", "code.exe", "msedgewebview2.exe", "photos.exe",
];

pub struct Scan {
    pub game: Option<String>,
    /// Refresh rate of the monitor the fullscreen game is on (None: background game).
    pub hz: Option<u32>,
    pub lhm_app_running: bool,
}

pub struct Detector {
    running: Option<String>,
    lhm_app: bool,
    scanned: Option<Instant>,
    me: u32,
    /// Last foreground pid and its exe name (lookup costs an OpenProcess).
    fg_cache: Option<(u32, Option<String>)>,
}

impl Detector {
    pub fn new() -> Self {
        Self { running: None, lhm_app: false, scanned: None, me: unsafe { GetCurrentProcessId() }, fg_cache: None }
    }

    pub fn tick(&mut self) -> Scan {
        // The process list changes slowly; walking it every 5s is plenty.
        if self.scanned.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
            self.scan_processes();
            self.scanned = Some(Instant::now());
        }
        let mut hz = None;
        let fg = self.foreground(&mut hz);
        Scan { game: fg.or_else(|| self.running.clone()), hz, lhm_app_running: self.lhm_app }
    }

    fn scan_processes(&mut self) {
        self.running = None;
        self.lhm_app = false;
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return };
            let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut ok = Process32FirstW(snap, &mut e).is_ok();
            while ok {
                let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                let name = String::from_utf16_lossy(&e.szExeFile[..n]);
                let low = name.to_lowercase();
                if self.running.is_none() && GAME_EXES.contains(&low.as_str()) {
                    self.running = Some(name);
                }
                if low == "librehardwaremonitor.exe" {
                    self.lhm_app = true;
                }
                ok = Process32NextW(snap, &mut e).is_ok();
            }
            let _ = CloseHandle(snap);
        }
    }

    /// Foreground window covering its whole monitor (not just the primary one).
    fn foreground(&mut self, hz: &mut Option<u32>) -> Option<String> {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_invalid() {
                return None;
            }
            let mut cls = [0u16; 64];
            let n = GetClassNameW(hwnd, &mut cls) as usize;
            let cls = String::from_utf16_lossy(&cls[..n]);
            if cls == "Progman" || cls == "WorkerW" {
                return None; // desktop
            }
            let mut r = RECT::default();
            GetWindowRect(hwnd, &mut r).ok()?;
            let mut mx = MONITORINFOEXW::default();
            mx.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            if !GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mx.monitorInfo).as_bool() {
                return None;
            }
            let m = mx.monitorInfo.rcMonitor;
            if !(r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom) {
                return None;
            }
            // A maximized window overhangs the work area by its borders, so on a
            // display without a taskbar it "covers" the monitor too: not a game.
            let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
            if IsZoomed(hwnd).as_bool() || style & WS_CAPTION.0 == WS_CAPTION.0 {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 || pid == self.me {
                return None;
            }
            if self.fg_cache.as_ref().is_none_or(|(p, _)| *p != pid) {
                let name = process_name(pid).filter(|n| !NOT_GAMES.contains(&n.to_lowercase().as_str()));
                self.fg_cache = Some((pid, name));
            }
            let name = self.fg_cache.as_ref().and_then(|(_, n)| n.clone());
            if name.is_some() {
                // FPS is judged against the game's own monitor, not the primary.
                let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
                if EnumDisplaySettingsW(PCWSTR(mx.szDevice.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm).as_bool() && dm.dmDisplayFrequency > 1 {
                    *hz = Some(dm.dmDisplayFrequency);
                }
            }
            name
        }
    }
}

fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 520];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        full.rsplit('\\').next().map(str::to_string)
    }
}
