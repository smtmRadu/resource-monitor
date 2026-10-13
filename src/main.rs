//! Resource Monitor (Rust port): transparent always-on-top pill monitor, docked upper-left.
//!
//! Window is click-through, so every control lives in the tray icon:
//! right click opens Hide/Show, Track, Size, Display, Update, Quit (left click does nothing).
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
mod update;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--selftest") => selftest::run(args.get(2).map(String::as_str).unwrap_or("selftest")),
        Some("--update") => std::process::exit(update::install()),
        Some("--apply") => update::apply_cli(&args[2..]),
        _ => app::run(),
    }
}
