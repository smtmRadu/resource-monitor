//! Linked rows: temporary displays pushed by other programs (Claude, training feeders).
//!
//! A program writes `<id>.json` into `%LOCALAPPDATA%\ResourceMonitor\links\`
//! (write a temp file, then rename it over the old one, so a half-written file
//! is never read); deleting the file removes the row. Format: README "Linked rows".
//!
//! The watcher thread blocks on a folder change notification, so with no links
//! it never wakes and the monitor costs exactly what it did before. While links
//! exist it also wakes every 15 s to drop expired ones.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use windows::core::HSTRING;
use windows::Win32::Foundation::{HWND, LPARAM, WAIT_OBJECT_0, WPARAM};
use windows::Win32::Storage::FileSystem::{
    FindFirstChangeNotificationW, FindNextChangeNotification, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
};
use windows::Win32::System::Threading::{WaitForSingleObject, INFINITE};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::log::log;

/// Finished / failed rows stay this long after their last write.
const TTL_DONE_S: u64 = 600;
/// A running row whose writer stopped updating (crashed, pod gone) is dropped after this.
const TTL_RUNNING_S: u64 = 1800;
const MAX_LINKS: usize = 4;
const MAX_POINTS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Done,
    Failed,
}

/// Vector icon on a 24x24 grid: optional rounded tile, then strokes.
#[derive(Clone, Debug)]
pub struct Icon {
    pub bg: Option<u32>,
    pub radius: f32,
    pub color: u32,
    pub width: f32,
    /// Polylines; `closed[i]` closes path i.
    pub paths: Vec<Vec<(f32, f32)>>,
    pub closed: Vec<bool>,
}

#[derive(Clone, Debug)]
pub struct Link {
    pub title: String,
    pub icon: Option<Icon>,
    /// Right pill text; None = built from step / total / ETA / state.
    pub text: Option<String>,
    pub step: Option<u64>,
    pub total: Option<u64>,
    /// ETA at `at`; the UI counts it down between writes.
    pub eta_s: Option<f32>,
    /// 0..1; defaults to step / total.
    pub progress: Option<f32>,
    /// Ribbon caption (left), e.g. the GPU type.
    pub caption: String,
    /// Graph metric name, shown with its last value on the ribbon (right).
    pub metric: String,
    /// Graph points: (x as a share of the run 0..1, value).
    pub points: Vec<(f32, f32)>,
    pub colors: (u32, u32),
    /// Snake length 0..1 around the title pill (e.g. the pod's GPU usage); None = no snake.
    pub snake: Option<f32>,
    pub state: State,
    /// Last write (file modified time).
    pub at: SystemTime,
}

impl Link {
    pub fn progress(&self) -> Option<f32> {
        self.progress
            .or_else(|| match (self.step, self.total) {
                (Some(s), Some(t)) if t > 0 => Some(s as f32 / t as f32),
                _ => None,
            })
            .map(|p| p.clamp(0.0, 1.0))
    }

    fn ttl_s(&self) -> u64 {
        if self.state == State::Running {
            TTL_RUNNING_S
        } else {
            TTL_DONE_S
        }
    }
}

static LATEST: Mutex<Option<Vec<Link>>> = Mutex::new(None);
static HWND_TARGET: AtomicIsize = AtomicIsize::new(0);

/// Fixed for every build (dev builds too), so writers never need to know which one runs.
pub fn dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("ResourceMonitor").join("links")
}

/// The current rows, once per change.
pub fn take_latest() -> Option<Vec<Link>> {
    LATEST.lock().ok()?.take()
}

/// Spawn the watcher; it posts `msg` to `hwnd` whenever the rows change.
pub fn start(hwnd: HWND, msg: u32) {
    HWND_TARGET.store(hwnd.0 as isize, Ordering::SeqCst);
    std::thread::Builder::new()
        .name("links".into())
        .spawn(move || watch(msg))
        .expect("links thread");
}

fn watch(msg: u32) {
    let dir = dir();
    let _ = std::fs::create_dir_all(&dir);
    let filter = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE;
    let Ok(h) = (unsafe { FindFirstChangeNotificationW(&HSTRING::from(dir.as_os_str()), false, filter) }) else {
        log("links: change notification failed; linked rows off");
        return;
    };
    let mut cache: HashMap<PathBuf, (SystemTime, u64, Link)> = HashMap::new();
    loop {
        if scan(&dir, &mut cache) {
            // By title, then file name: rows keep their place (rewrites never reorder them).
            let mut e: Vec<(&PathBuf, &Link)> = cache.iter().map(|(p, (_, _, l))| (p, l)).collect();
            e.sort_by(|a, b| (&a.1.title, a.0).cmp(&(&b.1.title, b.0)));
            let mut v: Vec<Link> = e.into_iter().map(|(_, l)| l.clone()).collect();
            v.truncate(MAX_LINKS);
            if let Ok(mut l) = LATEST.lock() {
                *l = Some(v);
            }
            let hw = HWND(HWND_TARGET.load(Ordering::SeqCst) as *mut _);
            unsafe {
                let _ = PostMessageW(Some(hw), msg, WPARAM(0), LPARAM(0));
            }
        }
        let timeout = if cache.is_empty() { INFINITE } else { 15_000 };
        if unsafe { WaitForSingleObject(h, timeout) } == WAIT_OBJECT_0 {
            // A rename-over fires a burst of notifications: let it settle, then re-arm.
            std::thread::sleep(Duration::from_millis(50));
            if unsafe { FindNextChangeNotification(h) }.is_err() {
                log("links: re-arm failed; linked rows off");
                return;
            }
        }
    }
}

/// Re-read changed files, drop removed / expired ones. True when anything changed.
fn scan(dir: &Path, cache: &mut HashMap<PathBuf, (SystemTime, u64, Link)>) -> bool {
    let now = SystemTime::now();
    let mut seen = Vec::new();
    let mut changed = false;
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(md) = e.metadata() else { continue };
        let at = md.modified().unwrap_or(now);
        seen.push(p.clone());
        if cache.get(&p).is_some_and(|c| c.0 == at && c.1 == md.len()) {
            continue;
        }
        // Unparsable (mid-write by a writer that skips the rename): retried on the next change.
        let Some(link) = std::fs::read(&p).ok().and_then(|b| parse(&b, at)) else {
            continue;
        };
        cache.insert(p, (at, md.len(), link));
        changed = true;
    }
    let before = cache.len();
    cache.retain(|p, _| seen.contains(p));
    changed |= cache.len() != before;
    cache.retain(|p, (at, _, l)| {
        let ttl = l.ttl_s();
        let alive = now.duration_since(*at).map_or(true, |d| d.as_secs() < ttl);
        if !alive {
            let _ = std::fs::remove_file(p);
            changed = true;
        }
        alive
    });
    changed
}

fn hex(v: Option<&Value>) -> Option<u32> {
    let s = v?.as_str()?.trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    u32::from_str_radix(s, 16).ok()
}

fn num(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

fn text(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

fn parse_icon(v: &Value) -> Option<Icon> {
    let mut paths = Vec::new();
    let mut closed = Vec::new();
    for (key, close) in [("lines", false), ("loops", true)] {
        for p in v.get(key).and_then(Value::as_array).into_iter().flatten() {
            let xs: Vec<f32> = p.as_array()?.iter().filter_map(Value::as_f64).map(|x| x as f32).collect();
            let pts: Vec<(f32, f32)> = xs.chunks_exact(2).map(|c| (c[0], c[1])).collect();
            if pts.len() >= 2 {
                paths.push(pts);
                closed.push(close);
            }
        }
    }
    Some(Icon {
        bg: hex(v.get("bg")),
        radius: num(v, "radius").unwrap_or(6.0) as f32,
        color: hex(v.get("color")).unwrap_or(0xffffff),
        width: num(v, "width").unwrap_or(2.0) as f32,
        paths,
        closed,
    })
}

fn parse(bytes: &[u8], at: SystemTime) -> Option<Link> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let total = num(&v, "total").filter(|t| *t > 0.0).map(|t| t as u64);
    let raw: Vec<(f32, f32)> = v
        .get("points")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let p = p.as_array()?;
            Some((p.first()?.as_f64()? as f32, p.get(1)?.as_f64()? as f32))
        })
        .filter(|p| p.0.is_finite() && p.1.is_finite())
        .collect();
    // x is a step: share of `total`, else of the largest x (the graph fills the ribbon).
    let span = total.map(|t| t as f32).unwrap_or_else(|| raw.iter().map(|p| p.0).fold(0.0, f32::max)).max(1e-6);
    let skip = raw.len().saturating_sub(MAX_POINTS);
    let points = raw[skip..].iter().map(|&(x, y)| ((x / span).clamp(0.0, 1.0), y)).collect();
    let colors = v.get("colors").and_then(Value::as_array);
    let c = |i: usize, d: u32| colors.and_then(|a| hex(a.get(i))).unwrap_or(d);
    Some(Link {
        title: text(&v, "title").unwrap_or_default(),
        icon: v.get("icon").and_then(parse_icon),
        text: text(&v, "text"),
        step: num(&v, "step").map(|s| s.max(0.0) as u64),
        total,
        eta_s: num(&v, "eta_s").map(|e| e as f32),
        progress: num(&v, "progress").map(|p| p as f32),
        caption: text(&v, "caption").unwrap_or_default(),
        metric: text(&v, "metric").unwrap_or_default(),
        points,
        colors: (c(0, 0x8b5cf6), c(1, 0x22d3ee)),
        snake: num(&v, "snake").map(|x| (x as f32).clamp(0.0, 1.0)),
        state: match v.get("state").and_then(Value::as_str) {
            Some("done") => State::Done,
            Some("failed") => State::Failed,
            _ => State::Running,
        },
        at,
    })
}

/// Parse link JSON written `age_s` seconds ago (self-test).
pub fn parse_str(json: &str, age_s: u64) -> Option<Link> {
    let at = SystemTime::now() - Duration::from_secs(age_s);
    parse(json.as_bytes(), at)
}
