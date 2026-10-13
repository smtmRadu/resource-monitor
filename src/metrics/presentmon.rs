//! Live FPS from a PresentMon child (ETW, needs elevation).
//!
//! Frames stream over the child's stdout as CSV; a reader thread keeps a
//! rolling window. FPS = 1000 / mean(MsBetweenPresents) over the last 2s of
//! the game's own swap chain: the one presenting most often, so launcher,
//! overlay or video swap chains of the same exe don't skew the number.
use std::collections::HashMap;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::jobs;

struct Capture {
    child: Child,
    game: String,
    frames: Arc<Mutex<VecDeque<Frame>>>,
}

#[derive(Clone, Copy)]
struct Frame {
    t: f64,  // TimeInSeconds
    ms: f64, // MsBetweenPresents
    chain: u64,
    arrived: Instant,
}

struct State {
    cap: Option<Capture>,
    last_try: Option<Instant>,
}

static STATE: Mutex<State> = Mutex::new(State { cap: None, last_try: None });

fn exe() -> Option<std::path::PathBuf> {
    let p = crate::log::home().join("vendor").join("PresentMon").join("PresentMon.exe");
    p.exists().then_some(p)
}

/// Killing PresentMon leaves its ETW session (and providers) running; ask
/// PresentMon to end it. `wait` on app exit, so the job doesn't kill it first.
fn end_session(wait: bool) {
    let Some(exe) = exe() else { return };
    let child = Command::new(exe)
        .arg("--terminate_existing_session")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(jobs::CREATE_NO_WINDOW)
        .spawn();
    if let (Ok(mut c), true) = (child, wait) {
        for _ in 0..30 {
            if !matches!(c.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = c.kill();
    }
}

fn kill(c: &mut Capture, wait: bool) {
    let _ = c.child.kill();
    let _ = c.child.wait();
    end_session(wait);
}

/// Game gone: stop capturing (no-op when nothing runs).
fn stop_capture() {
    if let Ok(mut s) = STATE.lock() {
        if let Some(mut c) = s.cap.take() {
            kill(&mut c, false);
        }
    }
}

/// App exit.
pub fn stop() {
    if let Ok(mut s) = STATE.lock() {
        if let Some(mut c) = s.cap.take() {
            kill(&mut c, true);
        }
    }
}

fn start(game: &str) -> Option<Capture> {
    let mut child = Command::new(exe()?)
        .args(["--process_name", game, "--output_stdout", "--v1_metrics", "--no_console_stats", "--stop_existing_session"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .creation_flags(jobs::CREATE_NO_WINDOW)
        .spawn()
        .ok()?;
    jobs::adopt(&child);
    let frames = Arc::new(Mutex::new(VecDeque::with_capacity(512)));
    let out = child.stdout.take()?;
    let sink = frames.clone();
    std::thread::spawn(move || {
        let (mut mi, mut ti, mut ci) = (usize::MAX, usize::MAX, usize::MAX);
        let mut rd = BufReader::new(out);
        let mut raw = Vec::new();
        loop {
            raw.clear();
            // Lossy decode: a non-UTF-8 byte (exe names in a legacy code page)
            // must not end the reader while the child keeps running.
            match rd.read_until(b'\n', &mut raw) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let line = String::from_utf8_lossy(&raw);
            let parts: Vec<&str> = line.trim().split(',').collect();
            if mi == usize::MAX {
                // v1 metrics spell it "msBetweenPresents"; match case-insensitively.
                let col = |n: &str| parts.iter().position(|h| h.trim().eq_ignore_ascii_case(n)).unwrap_or(usize::MAX);
                mi = col("MsBetweenPresents");
                ti = col("TimeInSeconds");
                ci = col("SwapChainAddress");
                if mi == usize::MAX || ti == usize::MAX {
                    mi = usize::MAX; // keep looking for the header
                }
                continue;
            }
            let (Some(t), Some(m)) = (parts.get(ti), parts.get(mi)) else { continue };
            let chain = parts
                .get(ci)
                .and_then(|c| u64::from_str_radix(c.trim().trim_start_matches("0x").trim_start_matches("0X"), 16).ok())
                .unwrap_or(0);
            if let (Ok(t), Ok(ms)) = (t.trim().parse::<f64>(), m.trim().parse::<f64>()) {
                if let Ok(mut f) = sink.lock() {
                    if f.len() >= 800 {
                        f.pop_front();
                    }
                    f.push_back(Frame { t, ms, chain, arrived: Instant::now() });
                }
            }
        }
    });
    Some(Capture { child, game: game.to_string(), frames })
}

/// Start/stop capture around the detected game; live FPS or None.
pub fn poll(game: Option<&str>) -> Option<f32> {
    let mut s = STATE.lock().ok()?;
    let Some(game) = game else {
        drop(s);
        stop_capture();
        return None;
    };
    let alive = s.cap.as_mut().is_some_and(|c| c.game == game && matches!(c.child.try_wait(), Ok(None)));
    if !alive {
        // Don't respawn-churn a capture that keeps dying.
        if s.last_try.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) {
            return None;
        }
        s.last_try = Some(Instant::now());
        if let Some(mut c) = s.cap.take() {
            let _ = c.child.kill(); // the new capture stops the old session itself
            let _ = c.child.wait();
        }
        s.cap = start(game);
        return None;
    }
    let f = s.cap.as_ref()?.frames.lock().ok()?;
    let last = *f.back()?;
    // Frames stopped (paused / minimized): show no number instead of a stale one.
    if last.arrived.elapsed() > Duration::from_secs(2) {
        return None;
    }
    // Per swap chain over the last 2s: (frame count, sum of frame times).
    let mut chains: HashMap<u64, (usize, f64)> = HashMap::new();
    for fr in f.iter().filter(|fr| last.t - fr.t <= 2.0 && fr.ms > 0.0) {
        let e = chains.entry(fr.chain).or_default();
        e.0 += 1;
        e.1 += fr.ms;
    }
    let &(n, sum) = chains.values().max_by_key(|(n, _)| *n)?;
    if n < 5 {
        return None;
    }
    let mean = sum / n as f64;
    Some(((1000.0 / mean) * 10.0).round() as f32 / 10.0)
}
