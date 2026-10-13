//! Resource Monitor (Rust port): transparent always-on-top pill monitor, docked upper-left.
//!
//! Window is click-through, so every control lives in the tray icon:
//! right click opens Hide/Show, Track, Size, Display, Quit (left click does nothing).
#![windows_subsystem = "windows"]

mod app;
mod draw;
mod log;
mod metrics;
mod selftest;
mod settings;
mod text;
mod tray;
mod ui;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--selftest") {
        selftest::run(args.get(2).map(String::as_str).unwrap_or("selftest"));
        return;
    }
    app::run();
}
