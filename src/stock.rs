//! Stock quote poller (NVDA, ANT): one background thread, one Yahoo quote request for all symbols.
//!
//! Yahoo's v7 quote API (free, no key) needs a session cookie plus a "crumb": the poller
//! gets both once (fc.yahoo.com sets the A3 cookie, /v1/test/getcrumb returns the crumb)
//! and fetches them again when a quote request is refused.
//! Every 10 s while the US market trades (pre-market + regular session), every
//! 5 min otherwise; a failed request retries after 30 s and keeps the last quotes.
//! Off (tray Stocks unchecked, the default) the thread idles: no requests.
//! One request returns both symbols (~1.5 KB + headers), so 10 s polling over the
//! 12 h of pre-market + session costs ~10 MB per trading day. The sampler copies the
//! latest quotes into each snapshot, so a slow request never stalls the 1 Hz metrics.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Networking::WinInet::*;

use crate::log::log;

/// Polled symbols; index k = stock group k (NVDA docks left, ANT right of it).
pub const SYMBOLS: [&str; 2] = ["NVDA", "ANT"];
pub const N: usize = SYMBOLS.len();

const COOKIE_URL: &str = "https://fc.yahoo.com";
const CRUMB_URL: &str = "https://query1.finance.yahoo.com/v1/test/getcrumb";
/// Only the fields `parse` reads (the reply shrinks to about half).
const QUOTE_URL: &str = "https://query1.finance.yahoo.com/v7/finance/quote?fields=regularMarketPrice,regularMarketPreviousClose,preMarketPrice,marketState";
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

pub type Quotes = [Option<Quote>; N];

static LATEST: Mutex<Quotes> = Mutex::new([None, None]);
static STOP: AtomicBool = AtomicBool::new(false);
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Turn polling on / off (off drops the last quotes, so a later start shows fresh data only).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::SeqCst);
    if !on {
        if let Ok(mut l) = LATEST.lock() {
            *l = Default::default();
        }
    }
}

pub fn latest() -> Quotes {
    LATEST.lock().map(|l| l.clone()).unwrap_or_default()
}

pub fn start() {
    let _ = std::thread::Builder::new().name("stock".into()).spawn(poller);
}

pub fn stop() {
    STOP.store(true, Ordering::SeqCst);
}

/// Yahoo session: the A3 cookie header line and the crumb that goes with it.
struct Auth {
    cookie: String,
    crumb: String,
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
    let url = format!("{QUOTE_URL}&symbols={}", SYMBOLS.join(","));
    let mut auth: Option<Auth> = None;
    let mut seq = 0u64;
    let mut fails = 0u32;
    while !STOP.load(Ordering::SeqCst) {
        if !ENABLED.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(250));
            continue;
        }
        let reply = match auth.take() {
            Some(a) => Ok(a),
            None => login(net),
        }
        .and_then(|a| {
            let r = get(net, &format!("{url}&crumb={}", a.crumb), Some(&a.cookie));
            // Refused (expired cookie / crumb): log in again on the next poll.
            if !matches!(r, Err(ref e) if e.starts_with("HTTP 401") || e.starts_with("HTTP 403")) {
                auth = Some(a);
            }
            r
        })
        .and_then(|(_, b)| parse(&b).ok_or_else(|| "unexpected reply".to_string()));
        let wait = match reply {
            Ok(q) => {
                seq += 1;
                if seq == 1 || fails > 0 {
                    let s: Vec<String> = SYMBOLS.iter().zip(&q).map(|(n, x)| match x {
                        Some((p, c, s)) => format!("{n} {p:.2} prev {c:.2} {s:?}"),
                        None => format!("{n} missing"),
                    }).collect();
                    log(&format!("stock: {}", s.join(", ")));
                }
                fails = 0;
                let trading = q.iter().flatten().any(|&(_, _, s)| s != Session::Closed);
                if let Ok(mut l) = LATEST.lock() {
                    for (slot, x) in l.iter_mut().zip(q) {
                        // A symbol missing from one reply keeps its last quote.
                        if let Some((price, prev_close, session)) = x {
                            *slot = Some(Quote { price, prev_close, session, seq });
                        }
                    }
                }
                if trading {
                    OPEN_EVERY
                } else {
                    CLOSED_EVERY
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

/// New Yahoo session: the cookie from fc.yahoo.com (it answers 404, with the cookie), then the crumb.
fn login(net: *mut core::ffi::c_void) -> Result<Auth, String> {
    let (cookies, _) = get(net, COOKIE_URL, None).or_else(|e| match e.split_once('\n') {
        Some((_, c)) => Ok((c.to_string(), Vec::new())), // non-200: the cookies ride in the error
        None => Err(e),
    })?;
    if cookies.is_empty() {
        return Err("no Yahoo cookie".into());
    }
    let cookie = format!("Cookie: {cookies}\r\n");
    let (_, body) = get(net, CRUMB_URL, Some(&cookie))?;
    let crumb = String::from_utf8_lossy(&body).trim().to_string();
    if crumb.is_empty() || crumb.contains(['{', '<', ' ']) {
        return Err("no Yahoo crumb".into());
    }
    Ok(Auth { cookie, crumb })
}

/// GET `url` (optional extra header lines): (the reply's cookies as "a=1; b=2", body).
/// A non-200 reply is an error "HTTP <code>", followed by "\n<cookies>" when it set some.
fn get(net: *mut core::ffi::c_void, url: &str, headers: Option<&str>) -> Result<(String, Vec<u8>), String> {
    unsafe {
        let flags = INTERNET_FLAG_RELOAD | INTERNET_FLAG_NO_CACHE_WRITE | INTERNET_FLAG_NO_UI | INTERNET_FLAG_NO_COOKIES | INTERNET_FLAG_KEEP_CONNECTION;
        let hdr: Option<Vec<u16>> = headers.map(|h| h.encode_utf16().collect());
        let req = InternetOpenUrlW(net, &HSTRING::from(url), hdr.as_deref(), flags, None);
        if req.is_null() {
            return Err("no connection".into());
        }
        let mut code = 0u32;
        let mut len = 4u32;
        let q = HttpQueryInfoW(req, HTTP_QUERY_STATUS_CODE | HTTP_QUERY_FLAG_NUMBER, Some(&mut code as *mut u32 as _), &mut len, None);
        let cookies = set_cookies(req);
        let out = if q.is_err() || code != 200 {
            Err(if cookies.is_empty() { format!("HTTP {code}") } else { format!("HTTP {code}\n{cookies}") })
        } else {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                let mut n = 0u32;
                if InternetReadFile(req, chunk.as_mut_ptr() as _, chunk.len() as u32, &mut n).is_err() {
                    break Err("read interrupted".to_string());
                }
                if n == 0 {
                    break Ok((cookies, buf));
                }
                buf.extend_from_slice(&chunk[..n as usize]);
            }
        };
        let _ = InternetCloseHandle(req);
        out
    }
}

/// The reply's Set-Cookie headers as one Cookie header value ("a=1; b=2"): name=value parts only.
unsafe fn set_cookies(req: *mut core::ffi::c_void) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut index = 0u32;
    loop {
        let mut buf = [0u16; 2048];
        let mut len = (buf.len() * 2) as u32;
        let before = index;
        if HttpQueryInfoW(req, HTTP_QUERY_SET_COOKIE, Some(buf.as_mut_ptr() as _), &mut len, Some(&mut index)).is_err() || index == before {
            break;
        }
        let line = String::from_utf16_lossy(&buf[..(len as usize / 2).min(buf.len())]);
        if let Some(pair) = line.split(';').next().map(str::trim).filter(|p| p.contains('=')) {
            out.push(pair.to_string());
        }
    }
    out.join("; ")
}

/// (price, previous close, session) per symbol, in `SYMBOLS` order.
fn parse(body: &[u8]) -> Option<[Option<(f64, f64, Session)>; N]> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let list = v["quoteResponse"]["result"].as_array()?;
    let one = |sym: &str| {
        let r = list.iter().find(|r| r["symbol"].as_str() == Some(sym))?;
        let regular = r["regularMarketPrice"].as_f64()?;
        match r["marketState"].as_str() {
            // regularMarketPrice is still yesterday's close: the reference of the pre-market move.
            Some("PRE") => Some((r["preMarketPrice"].as_f64().unwrap_or(regular), regular, Session::Pre)),
            state => {
                let prev = r["regularMarketPreviousClose"].as_f64()?;
                Some((regular, prev, if state == Some("REGULAR") { Session::Open } else { Session::Closed }))
            }
        }
    };
    let out = SYMBOLS.map(one);
    out.iter().any(Option::is_some).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live Yahoo round trip (network): `cargo test --release -- --ignored live_quote --nocapture`.
    #[test]
    #[ignore]
    fn live_quote() {
        let net = unsafe { InternetOpenW(w!("Mozilla/5.0 (Windows NT 10.0; Win64; x64) ResourceMonitor"), INTERNET_OPEN_TYPE_PRECONFIG.0, PCWSTR::null(), PCWSTR::null(), 0) };
        let a = login(net).expect("login");
        let url = format!("{QUOTE_URL}&symbols={}&crumb={}", SYMBOLS.join(","), a.crumb);
        let (_, body) = get(net, &url, Some(&a.cookie)).expect("quote");
        let q = parse(&body).expect("parse");
        println!("{} bytes: {q:?}", body.len());
        assert!(q.iter().all(Option::is_some));
    }
}
