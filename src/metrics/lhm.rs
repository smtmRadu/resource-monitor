//! CPU package temperature + power via LibreHardwareMonitor.
//!
//! LHM is a .NET library that needs admin for its ring-0 driver, so it runs in
//! an isolated PowerShell bridge (vendor/lhm_bridge.ps1) that writes lhm.json
//! every 2s. The bridge also falls back to the LHM app's WMI namespace.
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::jobs;

struct Bridge {
    child: Option<Child>,
    last_spawn: Option<Instant>,
}

static BRIDGE: Mutex<Bridge> = Mutex::new(Bridge { child: None, last_spawn: None });

fn json_path() -> std::path::PathBuf {
    crate::log::home().join("lhm.json")
}

/// Keep the bridge alive while it can produce data; back off 30s between
/// respawns (the Python supervisor respawned a failing bridge every second).
pub fn ensure(wanted: bool) {
    let Ok(mut b) = BRIDGE.lock() else { return };
    if !wanted {
        if let Some(mut c) = b.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        return;
    }
    if let Some(c) = b.child.as_mut() {
        if matches!(c.try_wait(), Ok(None)) {
            return;
        }
        b.child = None;
    }
    if b.last_spawn.is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
        return;
    }
    b.last_spawn = Some(Instant::now());
    let script = crate::log::home().join("vendor").join("lhm_bridge.ps1");
    if !script.exists() {
        return;
    }
    let child = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .arg("-Out")
        .arg(json_path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(jobs::CREATE_NO_WINDOW)
        .spawn();
    match child {
        Ok(c) => {
            jobs::adopt(&c);
            crate::log::log("lhm bridge started");
            b.child = Some(c);
        }
        Err(e) => crate::log::log(&format!("lhm bridge spawn failed: {e}")),
    }
}

pub fn stop() {
    ensure(false);
}

/// (temp °C, package W) from the bridge file; None when missing or stale (>10s).
pub fn read() -> (Option<f32>, Option<f32>) {
    let Ok(txt) = std::fs::read_to_string(json_path()) else { return (None, None) };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else { return (None, None) };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    if now - v.get("ts").and_then(|x| x.as_f64()).unwrap_or(0.0) > 10.0 {
        return (None, None);
    }
    let f = |k: &str| v.get(k).and_then(|x| x.as_f64()).map(|x| x as f32);
    // 0 means "no driver access" (not elevated), not a real reading.
    (f("temp").filter(|t| *t > 0.0), f("watts").filter(|w| *w > 0.0))
}
