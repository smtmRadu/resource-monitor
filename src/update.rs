//! Updates from the GitHub repo's `main` branch.
//!
//! `dist/manifest.json` on main lists every app file with its repo path and
//! sha256. Files that differ locally are downloaded into a staging folder,
//! hash-checked, then copied over the install folder.
//! - Background: a light check (the manifest only) 1 min after start, then
//!   hourly, 10 min after a failure (offline). It only flags the update; the
//!   tray menu then shows the new version next to the current one.
//! - Tray version item: checks now; installs when an update is flagged (or
//!   after a Yes). It stages in the background, starts `--apply` (elevated
//!   when the install folder is read-only) and quits; `--apply` waits for
//!   this process to exit, copies the files and restarts the app.
//! - Installer: it downloads only the exe, then runs `--update`, which stages
//!   and applies everything else (vendor/ sensor helpers) in one go.
use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::Networking::WinInet::*;
use windows::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::log::{home, log};
use crate::metrics::jobs::CREATE_NO_WINDOW;

const REPO: &str = "smtmRadu/resource-monitor";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Logon task created by setup_task.ps1 (and the installer).
const TASK: &str = "ResourceMonitorRs";
/// Inno Setup's uninstall entry (AppId in installer/setup.iss + "_is1").
const UNINSTALL_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{6F1C2E7A-3B9D-4C5E-8A21-9D4E7B0C5F13}_is1";
const FIRST_CHECK: Duration = Duration::from_secs(60);
const CHECK_EVERY: Duration = Duration::from_secs(3600);
const RETRY_OFFLINE: Duration = Duration::from_secs(600);
const TIMEOUT_MS: u32 = 15_000;

struct File {
    /// Install-relative path, `/`-separated.
    path: String,
    /// Repo path to download from.
    src: String,
    sha256: String,
}

struct Remote {
    version: String,
    /// Raw-file URL prefix, ends with `/`.
    base: String,
    files: Vec<File>,
}

struct State {
    /// Version on main when it differs from the installed files.
    latest: Option<String>,
    next_check: Option<Instant>,
    checking: bool,
}

static STATE: Mutex<State> = Mutex::new(State { latest: None, next_check: None, checking: false });
static BUSY: AtomicBool = AtomicBool::new(false);

fn get(url: &str, accept: Option<&str>) -> Result<Vec<u8>, String> {
    unsafe {
        let net = InternetOpenW(w!("ResourceMonitor"), INTERNET_OPEN_TYPE_PRECONFIG.0, PCWSTR::null(), PCWSTR::null(), 0);
        if net.is_null() {
            return Err("WinINet is not available".into());
        }
        for opt in [INTERNET_OPTION_CONNECT_TIMEOUT, INTERNET_OPTION_RECEIVE_TIMEOUT] {
            let _ = InternetSetOptionW(Some(net), opt, Some(&TIMEOUT_MS as *const u32 as _), 4);
        }
        let headers: Option<Vec<u16>> = accept.map(|a| format!("Accept: {a}\r\n").encode_utf16().collect());
        let flags = INTERNET_FLAG_RELOAD | INTERNET_FLAG_NO_CACHE_WRITE | INTERNET_FLAG_NO_UI | INTERNET_FLAG_NO_COOKIES;
        let req = InternetOpenUrlW(net, &HSTRING::from(url), headers.as_deref(), flags, None);
        let out = if req.is_null() { Err("No connection to GitHub. Check your internet connection.".into()) } else { read(req, url) };
        if !req.is_null() {
            let _ = InternetCloseHandle(req);
        }
        let _ = InternetCloseHandle(net);
        out
    }
}

unsafe fn read(req: *mut core::ffi::c_void, url: &str) -> Result<Vec<u8>, String> {
    let mut code = 0u32;
    let mut len = 4u32;
    let q = HttpQueryInfoW(req, HTTP_QUERY_STATUS_CODE | HTTP_QUERY_FLAG_NUMBER, Some(&mut code as *mut u32 as _), &mut len, None);
    if q.is_err() || code != 200 {
        return Err(format!("HTTP {code} for {url}"));
    }
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let mut n = 0u32;
        InternetReadFile(req, chunk.as_mut_ptr() as _, chunk.len() as u32, &mut n).map_err(|e| format!("Download interrupted: {e}"))?;
        if n == 0 {
            return Ok(buf);
        }
        buf.extend_from_slice(&chunk[..n as usize]);
    }
}

fn sha256(data: &[u8]) -> String {
    let mut out = [0u8; 32];
    unsafe {
        let _ = BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out);
    }
    out.iter().map(|b| format!("{b:02x}")).collect()
}

/// Hash of an installed file, cached by (size, mtime) so the hourly check
/// reads no file twice.
fn local_sha(p: &Path) -> Option<String> {
    type Cache = HashMap<PathBuf, (u64, SystemTime, String)>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let md = std::fs::metadata(p).ok()?;
    let key = (md.len(), md.modified().ok()?);
    let cache = CACHE.get_or_init(Default::default);
    if let Some((len, t, h)) = cache.lock().ok()?.get(p) {
        if (*len, *t) == key {
            return Some(h.clone());
        }
    }
    let h = sha256(&std::fs::read(p).ok()?);
    cache.lock().ok()?.insert(p.to_path_buf(), (key.0, key.1, h.clone()));
    Some(h)
}

/// Manifest paths must stay inside the install folder.
fn safe(path: &str) -> bool {
    !path.is_empty() && !path.contains(':') && !path.starts_with(['/', '\\']) && !path.split(['/', '\\']).any(|p| p == "..")
}

fn parse(body: &[u8], base: String) -> Result<Remote, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("Bad manifest: {e}"))?;
    let files = v["files"].as_array().ok_or("Bad manifest: no file list")?;
    let files = files
        .iter()
        .filter_map(|f| {
            let file = File {
                path: f["path"].as_str()?.to_string(),
                src: f["src"].as_str()?.to_string(),
                sha256: f["sha256"].as_str()?.to_ascii_lowercase(),
            };
            (safe(&file.path) && safe(&file.src)).then_some(file)
        })
        .collect();
    Ok(Remote { version: v["version"].as_str().unwrap_or("?").to_string(), base, files })
}

/// The manifest on main. `pin` resolves main to its current commit first, so
/// a download never mixes files from two pushes (raw.githubusercontent.com
/// caches each branch path for minutes); the hourly check skips that call.
/// `RM_UPDATE_BASE` (a raw-file URL prefix) overrides the source, to test a
/// release from a local server before pushing it.
fn remote(pin: bool) -> Result<Remote, String> {
    let base = match std::env::var("RM_UPDATE_BASE") {
        Ok(b) => b,
        Err(_) => {
            let sha = pin
                .then(|| get(&format!("https://api.github.com/repos/{REPO}/commits/main"), Some("application/vnd.github.sha")).ok())
                .flatten()
                .and_then(|b| String::from_utf8(b).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| s.len() == 40 && s.bytes().all(|c| c.is_ascii_hexdigit()))
                .unwrap_or_else(|| "main".into());
            format!("https://raw.githubusercontent.com/{REPO}/{sha}/")
        }
    };
    parse(&get(&format!("{base}dist/manifest.json"), None)?, base)
}

/// `v` is a newer version than this build (dotted numbers; same or older: no arrow).
fn newer(v: &str) -> bool {
    let parse = |s: &str| s.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    parse(v) > parse(VERSION)
}

/// Files that are missing or differ locally.
fn needed(r: &Remote) -> Vec<&File> {
    r.files.iter().filter(|f| local_sha(&home().join(&f.path)).as_deref() != Some(f.sha256.as_str())).collect()
}

/// Download `files` into a fresh staging folder, checking each hash.
fn stage(r: &Remote, files: &[&File]) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("ResourceMonitorUpdate");
    let _ = std::fs::remove_dir_all(&dir);
    for f in files {
        let data = get(&format!("{}{}", r.base, f.src), None)?;
        if sha256(&data) != f.sha256 {
            return Err(format!("{} is corrupted (hash mismatch). Try again in a few minutes.", f.src));
        }
        let dst = dir.join(&f.path);
        if let Some(p) = dst.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        std::fs::write(&dst, data).map_err(|e| format!("{}: {e}", dst.display()))?;
    }
    log(&format!("update {}: staged {} file(s)", r.version, files.len()));
    Ok(dir)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// Copy the staged files over the install folder. A file in use (the running
/// exe) is renamed to `*.old` first; `cleanup` deletes those on the next start.
/// Copies (not moves), so new files get the install folder's permissions.
fn apply(dir: &Path) -> Result<usize, String> {
    let mut files = Vec::new();
    walk(dir, &mut files);
    for src in &files {
        let rel = src.strip_prefix(dir).map_err(|e| e.to_string())?;
        let dst = home().join(rel);
        if let Some(p) = dst.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        if dst.exists() && std::fs::remove_file(&dst).is_err() {
            let old = PathBuf::from(format!("{}.old", dst.display()));
            let _ = std::fs::remove_file(&old);
            std::fs::rename(&dst, &old).map_err(|e| format!("{} is in use: {e}", dst.display()))?;
        }
        std::fs::copy(src, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    }
    let _ = std::fs::remove_dir_all(dir);
    log(&format!("update: applied {} file(s)", files.len()));
    Ok(files.len())
}

/// Delete `*.old` files left by an update that replaced the running exe.
pub fn cleanup() {
    if crate::log::dev() {
        return;
    }
    let mut files = Vec::new();
    walk(home(), &mut files);
    for f in files.iter().filter(|f| f.extension().is_some_and(|e| e == "old")) {
        let _ = std::fs::remove_file(f);
    }
}

/// Keep "Apps & features" showing the installed version (the installer only
/// bootstraps, so it cannot know it). Needs admin; skipped silently otherwise.
fn set_display_version(v: &str) {
    let data: Vec<u8> = v.encode_utf16().chain(Some(0)).flat_map(u16::to_le_bytes).collect();
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_LOCAL_MACHINE, &HSTRING::from(UNINSTALL_KEY), None, KEY_SET_VALUE, &mut key).is_ok() {
            let _ = RegSetValueExW(key, w!("DisplayVersion"), None, REG_SZ, Some(&data));
            let _ = RegCloseKey(key);
        }
    }
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(".write-test");
    let ok = std::fs::OpenOptions::new().write(true).create_new(true).open(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

fn message(text: &str, error: bool) {
    let icon = if error { MB_ICONERROR } else { MB_ICONINFORMATION };
    unsafe {
        MessageBoxW(None, &HSTRING::from(text), w!("Resource Monitor"), MB_OK | icon | MB_TOPMOST | MB_SETFOREGROUND);
    }
}

fn ask(text: &str) -> bool {
    let r = unsafe { MessageBoxW(None, &HSTRING::from(text), w!("Resource Monitor"), MB_YESNO | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND) };
    r == IDYES
}

/// Version on main when an update is available.
pub fn latest() -> Option<String> {
    STATE.lock().ok()?.latest.clone()
}

pub fn tip() -> String {
    match latest() {
        Some(v) => format!("Resource Monitor {VERSION} (update {v} available)"),
        None => format!("Resource Monitor {VERSION}"),
    }
}

fn set_latest(hwnd: HWND, latest: Option<String>) {
    let changed = STATE.lock().map(|mut s| std::mem::replace(&mut s.latest, latest.clone()) != latest).unwrap_or(false);
    if changed {
        log(&format!("update available: {latest:?}"));
        crate::tray::set_tip(hwnd);
    }
}

/// Called every few seconds from the UI thread; starts the background check
/// when it is due. Never installs anything.
pub fn tick(hwnd: HWND) {
    if crate::log::dev() {
        return;
    }
    let now = Instant::now();
    {
        let Ok(mut s) = STATE.lock() else { return };
        let due = *s.next_check.get_or_insert(now + FIRST_CHECK);
        if s.checking || now < due {
            return;
        }
        s.checking = true;
    }
    let h = hwnd.0 as isize;
    std::thread::spawn(move || {
        // Only a newer version flags the arrow; same-version file differences are a
        // repair, offered when the version item is clicked.
        let r = remote(false).map(|r| (newer(&r.version) && !needed(&r).is_empty()).then(|| r.version.clone()));
        if let Ok(latest) = &r {
            set_latest(HWND(h as *mut _), latest.clone());
        }
        if let Ok(mut s) = STATE.lock() {
            s.checking = false;
            s.next_check = Some(Instant::now() + if r.is_ok() { CHECK_EVERY } else { RETRY_OFFLINE });
        }
    });
}

/// Tray version item: check now, and install when an update is available.
/// On install, post `quit_cmd` to `hwnd` so this instance exits and
/// `--apply` can replace its files.
pub fn click(hwnd: HWND, cmd_msg: u32, quit_cmd: u32) {
    if BUSY.swap(true, Ordering::AcqRel) {
        return;
    }
    let h = hwnd.0 as isize;
    std::thread::spawn(move || {
        let hwnd = HWND(h as *mut _);
        match manual(hwnd) {
            Ok(Some(v)) => {
                log(&format!("update to {v}: restarting"));
                unsafe {
                    let _ = PostMessageW(Some(hwnd), cmd_msg, WPARAM(quit_cmd as usize), LPARAM(0));
                }
            }
            Ok(None) => {}
            Err(e) => {
                log(&format!("update failed: {e}"));
                message(&format!("Update failed.\n\n{e}"), true);
            }
        }
        BUSY.store(false, Ordering::Release);
    });
}

/// Ok(Some(version)) when an update is staged and `--apply` is running.
fn manual(hwnd: HWND) -> Result<Option<String>, String> {
    if crate::log::dev() {
        return Err("This is a dev build: update it with git pull and cargo build.".into());
    }
    let flagged = latest().is_some();
    let r = remote(true)?;
    let files = needed(&r);
    set_latest(hwnd, (newer(&r.version) && !files.is_empty()).then(|| r.version.clone()));
    if files.is_empty() {
        message(&format!("Resource Monitor {VERSION} is up to date."), false);
        return Ok(None);
    }
    // A flagged update (green arrow) installs on click; a fresh find asks first.
    if !flagged {
        let q = if !newer(&r.version) {
            "Some Resource Monitor files are missing or damaged.\n\nRepair now? It restarts in a few seconds.".to_string()
        } else {
            format!("Resource Monitor {} is available (you have {VERSION}).\n\nUpdate now? It restarts in a few seconds.", r.version)
        };
        if !ask(&q) {
            return Ok(None);
        }
    }
    let dir = stage(&r, &files)?;
    launch_apply(&dir, &r.version)?;
    Ok(Some(r.version))
}

/// Start `--apply` for the staged files: directly when the install folder is
/// writable, else elevated (UAC prompt).
fn launch_apply(dir: &Path, version: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let pid = std::process::id().to_string();
    if writable(home()) {
        Command::new(&exe)
            .arg("--apply")
            .arg(dir)
            .args(["--pid", &pid, "--version", version, "--relaunch"])
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    let params = format!("--apply \"{}\" --pid {pid} --version {version} --relaunch", dir.display());
    let r = unsafe { ShellExecuteW(None, w!("runas"), &HSTRING::from(exe.as_os_str()), &HSTRING::from(params), PCWSTR::null(), SW_SHOWNORMAL) };
    if r.0 as isize <= 32 {
        return Err("Update cancelled: administrator approval is needed to update the installed files.".into());
    }
    Ok(())
}

/// `--update` (run by the installer): bring the install folder up to date
/// with main. Exit code 0 = up to date now.
pub fn install() -> i32 {
    let r = remote(true).and_then(|r| {
        let files = needed(&r);
        if !files.is_empty() {
            apply(&stage(&r, &files)?)?;
        }
        Ok((r.version.clone(), files.len()))
    });
    match r {
        Ok((v, n)) => {
            set_display_version(&v);
            log(&format!("--update: {v}, {n} file(s) updated"));
            0
        }
        Err(e) => {
            log(&format!("--update failed: {e}"));
            1
        }
    }
}

/// `--apply <dir> --pid <pid> --version <v> [--relaunch]`: wait for the tray
/// instance to exit, copy the staged files, restart the app.
pub fn apply_cli(args: &[String]) {
    let Some(dir) = args.first() else { return };
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1));
    let exe = std::env::current_exe().ok();
    if let Some(pid) = opt("--pid").and_then(|p| p.parse::<u32>().ok()) {
        unsafe {
            if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
                WaitForSingleObject(h, 15_000);
                let _ = CloseHandle(h);
            }
        }
    }
    match apply(Path::new(dir)) {
        Ok(_) => {
            if let Some(v) = opt("--version") {
                set_display_version(v);
            }
        }
        Err(e) => {
            log(&format!("--apply failed: {e}"));
            message(&format!("Update failed.\n\n{e}"), true);
        }
    }
    if args.iter().any(|a| a == "--relaunch") {
        // The logon task starts the app elevated (FPS + exact CPU sensors).
        let task = Command::new("schtasks").args(["/run", "/tn", TASK]).creation_flags(CREATE_NO_WINDOW).status();
        if !task.is_ok_and(|s| s.success()) {
            if let Some(exe) = exe {
                let _ = Command::new(exe).spawn();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vector() {
        assert_eq!(sha256(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn manifest_rejects_paths_outside_install() {
        let body = br#"{"version":"1.0.0","files":[
            {"path":"a.exe","src":"dist/a.exe","sha256":"AB"},
            {"path":"../evil.exe","src":"x","sha256":"00"},
            {"path":"C:/evil.exe","src":"x","sha256":"00"},
            {"path":"vendor/x.dll","src":"/abs","sha256":"00"}]}"#;
        let r = parse(body, String::new()).unwrap();
        assert_eq!(r.version, "1.0.0");
        assert_eq!(r.files.len(), 1);
        assert_eq!(r.files[0].sha256, "ab");
    }
}
