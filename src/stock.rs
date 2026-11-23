//! NVDA quote poller: one background thread, Yahoo's chart API (free, no key).
//!
//! Every 10 s while the US market trades (pre-market + regular session), every
//! 5 min otherwise; a failed request retries after 30 s and keeps the last quote.
//! Off (tray Stocks > NVDA unchecked, the default) the thread idles: no requests.
//! One request asks for the last 2 min of 1-minute bars (~1.5 KB + headers), so
//! 10 s polling over the 12 h of pre-market + session costs ~10 MB per trading day. The sampler copies the latest quote into each
//! snapshot, so a slow request never stalls the 1 Hz metrics.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Networking::WinInet::*;

use crate::log::log;

/// Minute bars with pre/post-market; the window (period1/period2) is appended per request.
const URL: &str = "https://query1.finance.yahoo.com/v8/finance/chart/NVDA?interval=1m&includePrePost=true";
const OPEN_EVERY: Duration = Duration::from_secs(10);
const CLOSED_EVERY: Duration = Duration::from_secs(300);
const RETRY: Duration = Duration::from_secs(30);
const TIMEOUT_MS: u32 = 10_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Session {
    /// 4:00-9:30 ET: the price is the last pre-market trade, the reference yesterday's close.
    Pre,
    /// Regular session.
    Open,
    #[default]
    Closed,
}

#[derive(Clone, Debug, Default)]
pub struct Quote {
    pub price: f64,
    /// Previous session's close: the reference of the day change.
    pub prev_close: f64,
    pub session: Session,
    /// Bumped on every successful poll (the UI adds a ribbon point per new seq).
    pub seq: u64,
}

impl Quote {
    /// Day change in percent.
    pub fn change_pct(&self) -> f64 {
        if self.prev_close > 0.0 {
            (self.price / self.prev_close - 1.0) * 100.0
        } else {
            0.0
        }
    }
}

static LATEST: Mutex<Option<Quote>> = Mutex::new(None);
static STOP: AtomicBool = AtomicBool::new(false);
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Turn polling on / off (off drops the last quote, so a later start shows fresh data only).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::SeqCst);
    if !on {
        if let Ok(mut l) = LATEST.lock() {
            *l = None;
        }
    }
}

pub fn latest() -> Option<Quote> {
    LATEST.lock().ok()?.clone()
}

pub fn start() {
    let _ = std::thread::Builder::new().name("stock".into()).spawn(poller);
}

pub fn stop() {
    STOP.store(true, Ordering::SeqCst);
}

fn poller() {
    // One WinINet session for the whole run: requests reuse its keep-alive connection.
    let net = unsafe {
        InternetOpenW(w!("Mozilla/5.0 (Windows NT 10.0; Win64; x64) ResourceMonitor"), INTERNET_OPEN_TYPE_PRECONFIG.0, PCWSTR::null(), PCWSTR::null(), 0)
    };
    if net.is_null() {
        log("stock: WinINet unavailable");
        return;
    }
    for opt in [INTERNET_OPTION_CONNECT_TIMEOUT, INTERNET_OPTION_RECEIVE_TIMEOUT] {
        let _ = unsafe { InternetSetOptionW(Some(net), opt, Some(&TIMEOUT_MS as *const u32 as _), 4) };
    }
    let mut seq = 0u64;
    let mut fails = 0u32;
    while !STOP.load(Ordering::SeqCst) {
        if !ENABLED.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(250));
            continue;
        }
        let wait = match get(net).and_then(|b| parse(&b).ok_or_else(|| "unexpected reply".to_string())) {
            Ok((price, prev_close, session)) => {
                seq += 1;
                if seq == 1 || fails > 0 {
                    log(&format!("stock: NVDA {price:.2} prev {prev_close:.2} {session:?}"));
                }
                fails = 0;
                if let Ok(mut l) = LATEST.lock() {
                    *l = Some(Quote { price, prev_close, session, seq });
                }
                if session == Session::Closed {
                    CLOSED_EVERY
                } else {
                    OPEN_EVERY
                }
            }
            Err(e) => {
                fails += 1;
                if fails <= 3 {
                    log(&format!("stock: {e}"));
                }
                RETRY
            }
        };
        // Sleep in short steps so Quit never waits on this thread.
        let until = Instant::now() + wait;
        while Instant::now() < until && !STOP.load(Ordering::SeqCst) && ENABLED.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    let _ = unsafe { InternetCloseHandle(net) };
}

fn get(net: *mut core::ffi::c_void) -> Result<Vec<u8>, String> {
    unsafe {
        let flags = INTERNET_FLAG_RELOAD | INTERNET_FLAG_NO_CACHE_WRITE | INTERNET_FLAG_NO_UI | INTERNET_FLAG_NO_COOKIES | INTERNET_FLAG_KEEP_CONNECTION;
        let now = unix_now();
        let url = format!("{URL}&period1={}&period2={now}", now - 120);
        let req = InternetOpenUrlW(net, &HSTRING::from(url), None, flags, None);
        if req.is_null() {
            return Err("no connection".into());
        }
        let mut code = 0u32;
        let mut len = 4u32;
        let q = HttpQueryInfoW(req, HTTP_QUERY_STATUS_CODE | HTTP_QUERY_FLAG_NUMBER, Some(&mut code as *mut u32 as _), &mut len, None);
        let out = if q.is_err() || code != 200 {
            Err(format!("HTTP {code}"))
        } else {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                let mut n = 0u32;
                if InternetReadFile(req, chunk.as_mut_ptr() as _, chunk.len() as u32, &mut n).is_err() {
                    break Err("read interrupted".to_string());
                }
                if n == 0 {
                    break Ok(buf);
                }
                buf.extend_from_slice(&chunk[..n as usize]);
            }
        };
        let _ = InternetCloseHandle(req);
        out
    }
}

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// (price, previous close, session) from the chart reply.
fn parse(body: &[u8]) -> Option<(f64, f64, Session)> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let r = &v["chart"]["result"][0];
    let m = &r["meta"];
    let regular = m["regularMarketPrice"].as_f64()?;
    let now = unix_now();
    let inside = |k: &str| {
        let t = &m["currentTradingPeriod"][k];
        matches!((t["start"].as_i64(), t["end"].as_i64()), (Some(a), Some(b)) if now >= a && now < b)
    };
    if inside("pre") {
        // regularMarketPrice is still yesterday's close; the live price is the newest bar.
        let last = r["indicators"]["quote"][0]["close"].as_array()?.iter().rev().find_map(Value::as_f64);
        return Some((last.unwrap_or(regular), regular, Session::Pre));
    }
    let prev = m["previousClose"].as_f64().or_else(|| m["chartPreviousClose"].as_f64())?;
    Some((regular, prev, if inside("regular") { Session::Open } else { Session::Closed }))
}
