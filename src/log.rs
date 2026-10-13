//! Tiny append-only log in the data dir, capped so it can never grow without bound.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use windows::Win32::System::SystemInformation::GetLocalTime;

const MAX_BYTES: u64 = 512 * 1024;
static LOCK: Mutex<()> = Mutex::new(());

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Dev build: the exe runs from the repo's target/{release,debug}.
pub fn dev() -> bool {
    let d = exe_dir();
    let parent = d.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str());
    parent == Some("target") && matches!(d.file_name().and_then(|n| n.to_str()), Some("release" | "debug"))
}

/// App home (holds `vendor/`): the repo root for dev builds, else the install folder.
pub fn home() -> &'static PathBuf {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let d = exe_dir();
        if dev() {
            if let Some(root) = d.parent().and_then(|p| p.parent()) {
                return root.to_path_buf();
            }
        }
        d
    })
}

/// Writable files (settings, log, sensor bridge output): the repo root for dev
/// builds, else %LOCALAPPDATA%\ResourceMonitor (Program Files is read-only
/// unless elevated).
pub fn data_dir() -> &'static PathBuf {
    static DATA: OnceLock<PathBuf> = OnceLock::new();
    DATA.get_or_init(|| {
        if !dev() {
            if let Some(base) = std::env::var_os("LOCALAPPDATA") {
                let d = PathBuf::from(base).join("ResourceMonitor");
                if fs::create_dir_all(&d).is_ok() {
                    return d;
                }
            }
        }
        home().clone()
    })
}

pub fn log(msg: &str) {
    let _g = LOCK.lock();
    let path = data_dir().join("app.log");
    if fs::metadata(&path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = fs::rename(&path, data_dir().join("app.log.old"));
    }
    let t = unsafe { GetLocalTime() };
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{:02}:{:02}:{:02} {}", t.wHour, t.wMinute, t.wSecond, msg);
    }
}
