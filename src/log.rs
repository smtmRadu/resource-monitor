//! Tiny append-only log next to the app, capped so it can never grow without bound.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use windows::Win32::System::SystemInformation::GetLocalTime;

const MAX_BYTES: u64 = 512 * 1024;
static LOCK: Mutex<()> = Mutex::new(());

/// Project home: the first ancestor of the exe that holds `vendor/` (dev builds
/// live in target/release), else the exe's own folder.
pub fn home() -> &'static PathBuf {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        let mut d = Some(exe_dir.as_path());
        for _ in 0..4 {
            match d {
                Some(p) if p.join("vendor").is_dir() => return p.to_path_buf(),
                Some(p) => d = p.parent(),
                None => break,
            }
        }
        exe_dir
    })
}

pub fn log(msg: &str) {
    let _g = LOCK.lock();
    let path = home().join("app.log");
    if fs::metadata(&path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let _ = fs::rename(&path, home().join("app.log.old"));
    }
    let t = unsafe { GetLocalTime() };
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{:02}:{:02}:{:02} {}", t.wHour, t.wMinute, t.wSecond, msg);
    }
}
