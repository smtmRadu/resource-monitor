//! Off-screen self-test: `resource-monitor-rs.exe --selftest <dir>`.
//!
//! Renders frames to raw RGBA files (`<name>_<w>x<h>.rgba`, premultiplied)
//! without opening a window: one from live metrics, the rest from synthetic
//! snapshots that exercise every element (temps, fire, iGPU, game/FPS row,
//! scales, track off) and the tray version item. Also writes timing info to selftest.txt.
use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{FILETIME, HWND};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

use crate::metrics::{self, Gpu, Snapshot};
use crate::stock::{Quote, Session};
use crate::ui::{Group, Ui, FLAME_FPS};

fn save(ui: &mut Ui, dir: &Path, name: &str, report: &mut String) {
    let t = Instant::now();
    ui.invalidate();
    let mut f = ui.base().clone();
    let ov = ui.overlay().clone();
    f.draw_pixmap(0, 0, ov.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
    for px in f.data_mut().as_chunks_mut::<4>().0 {
        px.swap(0, 2); // layers are BGRA; the dump is RGBA
    }
    let el = t.elapsed();
    let path = dir.join(format!("{name}_{}x{}.rgba", f.width(), f.height()));
    let _ = std::fs::write(path, f.data());
    let _ = writeln!(report, "{name}: {}x{} full render {:?}", f.width(), f.height(), el);
}

/// This process's CPU time (kernel + user), ms.
fn cpu_ms() -> u64 {
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    let _ = unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) };
    let ms = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) / 10_000;
    ms(k) + ms(u)
}

fn synth(game: bool) -> Snapshot {
    Snapshot {
        cpu: 63.0,
        cores: (0..16).map(|i| (i * 7 % 100) as f32).collect(),
        ram_pct: 71.0,
        ram_used_gb: 22.3,
        ram_total_gb: 31.3,
        gpu: Gpu {
            util: Some(97.0),
            vram_used_mb: Some(6400.0),
            vram_total_mb: Some(8188.0),
            temp: Some(101.0),
            watts: Some(78.4),
            limit_w: Some(80.0),
        },
        igpu: Some(4.2),
        cpu_temp: Some(72.0),
        cpu_watts: Some(35.2),
        game: game.then(|| "Cyberpunk2077.exe".to_string()),
        fps: game.then_some(58.7),
        hz: 60,
        disk_mbs: 120.5,
        drives: vec![
            metrics::Drive { letter: 'C', used: 0.93, total_bytes: 1 << 40, disk: 0 },
            metrics::Drive { letter: 'D', used: 0.35, total_bytes: 1 << 34, disk: 0 },
            metrics::Drive { letter: 'E', used: 0.62, total_bytes: 1 << 39, disk: 1 },
        ],
        net_down: 12.3,
        net_up: 1.4,
        stock: Default::default(),
    }
}

pub fn run(dir: &str) {
    let dir = Path::new(dir);
    let _ = std::fs::create_dir_all(dir);
    let mut report = String::new();
    let info = metrics::info();
    let mut ui = Ui::new(info.clone(), 1.0, true, Group::System);

    // Stock group: 60 synthetic polls (random walk), market open (up) and closed (down).
    let mut st = Ui::new(info.clone(), 1.0, true, Group::Stock(0));
    let mut price = 180.0f64;
    for seq in 1..=61u64 {
        price += ((seq * 7919 % 13) as f64 - 5.5) * 0.03;
        st.apply(&Snapshot { stock: [Some(Quote { price, prev_close: 178.0, session: Session::Open, seq }), None, None], ..synth(false) });
    }
    st.step(0.6);
    save(&mut st, dir, "stock_up", &mut report);
    st.apply(&Snapshot { stock: [Some(Quote { price, prev_close: 176.0, session: Session::Pre, seq: 98 }), None, None], ..synth(false) });
    st.base();
    st.step(0.6);
    save(&mut st, dir, "stock_pre", &mut report);
    st.apply(&Snapshot { stock: [Some(Quote { price, prev_close: 186.0, session: Session::Closed, seq: 99 }), None, None], ..synth(true) });
    st.base(); // starts the values crossfade
    st.step(0.6); // ... and runs it to the end
    save(&mut st, dir, "stock_down_closed_game", &mut report);
    let _ = writeln!(report, "stock visible top/bottom {}/{}", st.visible_top(), st.visible_bottom());
    // ANT group (index 1): same snapshots carry both quotes; it reads only its own.
    let mut ant = Ui::new(info.clone(), 1.0, true, Group::Stock(1));
    let mut ap = 88.9f64;
    for seq in 1..=61u64 {
        ap -= ((seq * 6007 % 11) as f64 - 4.5) * 0.02;
        let nv = Quote { price: 180.0, prev_close: 178.0, session: Session::Open, seq };
        ant.apply(&Snapshot { stock: [Some(nv), Some(Quote { price: ap, prev_close: 88.91, session: Session::Open, seq }), None], ..synth(false) });
    }
    ant.step(0.6);
    save(&mut ant, dir, "stock_ant", &mut report);
    // AMD group (index 2, docked under NVDA): a falling day.
    let mut amd = Ui::new(info, 1.0, true, Group::Stock(2));
    let mut mp = 152.0f64;
    for seq in 1..=61u64 {
        mp += ((seq * 104729 % 11) as f64 - 5.4) * 0.04;
        amd.apply(&Snapshot { stock: [None, None, Some(Quote { price: mp, prev_close: 155.3, session: Session::Open, seq })], ..synth(false) });
    }
    amd.step(0.6);
    save(&mut amd, dir, "stock_amd", &mut report);

    // Live metrics through the real sampler (no window to notify).
    metrics::start(HWND(std::ptr::null_mut()), 0);
    let t = Instant::now();
    let mut n = 0;
    let want: usize = std::env::var("RM_SAMPLES").ok().and_then(|v| v.parse().ok()).unwrap_or(6);
    while n < want && t.elapsed() < Duration::from_secs(60) {
        if let Some(s) = metrics::take_latest() {
            ui.apply(&s);
            n += 1;
            let _ = writeln!(report, "live {n}: {s:?}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    ui.step(0.5);
    save(&mut ui, dir, "live", &mut report);

    // Hidden window: the sampler parks, so the process does no work.
    metrics::set_paused(true);
    std::thread::sleep(Duration::from_millis(1500)); // it notices within one tick
    let _ = metrics::take_latest();
    let c0 = cpu_ms();
    std::thread::sleep(Duration::from_secs(5));
    let (paused, stale) = (cpu_ms() - c0, metrics::take_latest().is_some());
    metrics::set_paused(false);
    let (c0, t) = (cpu_ms(), Instant::now());
    let mut k = 0;
    while t.elapsed() < Duration::from_secs(5) {
        k += metrics::take_latest().is_some() as u32;
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = writeln!(report, "cpu over 5s: paused {paused} ms (sample while paused: {stale}), running {} ms, {k} samples after resume", cpu_ms() - c0);

    // Synthetic: every element on, snakes rotated a bit.
    for _ in 0..8 {
        ui.apply(&synth(false));
    }
    for _ in 0..20 {
        ui.step(0.042);
    }
    save(&mut ui, dir, "synth", &mut report);
    // Fire glyph (GPU at 101°): consecutive flicker frames.
    let frames: usize = std::env::var("RM_FIRE_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    for i in 0..frames {
        ui.step(1.0 / FLAME_FPS);
        save(&mut ui, dir, &format!("fire{i:03}"), &mut report);
    }
    ui.apply(&synth(true));
    ui.step(0.4);
    save(&mut ui, dir, "game", &mut report);
    // Uncapped game above a 165 Hz monitor: yellow FPS value.
    ui.apply(&Snapshot { fps: Some(210.0), hz: 165, ..synth(true) });
    ui.base(); // starts the values crossfade
    ui.step(0.35); // ... and runs it to the end
    save(&mut ui, dir, "game_over", &mut report);
    // Game ended: FPS row halfway through its collapse.
    ui.apply(&synth(false));
    ui.step(0.15);
    save(&mut ui, dir, "collapse", &mut report);
    ui.step(0.3);
    ui.apply(&synth(true));
    ui.track = false;
    save(&mut ui, dir, "notrack", &mut report);
    ui.track = true;
    ui.apply(&synth(false));
    ui.set_scale(0.75);
    save(&mut ui, dir, "scale075", &mut report);
    ui.set_scale(1.25);
    save(&mut ui, dir, "scale125", &mut report);

    // Animation frame cost at 1x (snake layer only).
    ui.set_scale(1.0);
    ui.overlay();
    let t = Instant::now();
    for _ in 0..200 {
        ui.step(0.042);
        ui.overlay();
    }
    let _ = writeln!(report, "anim frame avg {:?}", t.elapsed() / 200);
    // Ribbon scroll frame cost at 1x (static layer copy + lines + captions), 30 fps.
    ui.apply(&synth(false));
    ui.base();
    let t = Instant::now();
    for _ in 0..29 {
        ui.step(1.0 / 30.0);
        ui.base();
    }
    let _ = writeln!(report, "lines frame avg {:?}", t.elapsed() / 29);
    // Same with the values crossfade running (first FADE_S after a sample).
    let t = Instant::now();
    for _ in 0..20 {
        ui.apply(&synth(false));
        ui.base();
        ui.step(1.0 / 30.0);
        ui.base();
    }
    let _ = writeln!(report, "sample + fade frame avg {:?}", t.elapsed() / 20);
    // Flame alone (track off, FLAME_FPS timer): static layer copy + rings per frame.
    ui.track = false;
    ui.invalidate();
    ui.base();
    let t = Instant::now();
    for _ in 0..24 {
        ui.step(1.0 / FLAME_FPS);
        ui.base();
    }
    let _ = writeln!(report, "flame frame avg {:?} at {FLAME_FPS} fps", t.elapsed() / 24);
    ui.track = true;
    // Window hidden 15 s: hatched span on every ribbon, then the lines resume.
    for k in 0..30 {
        ui.apply(&Snapshot { cpu: 40.0 + 30.0 * (k as f32 * 0.5).sin(), ..synth(false) });
    }
    ui.gap(15);
    save(&mut ui, dir, "gap_open", &mut report);
    for k in 0..10 {
        ui.apply(&Snapshot { cpu: 50.0 + 25.0 * (k as f32 * 0.7).cos(), ..synth(false) });
    }
    ui.step(0.5);
    save(&mut ui, dir, "gap", &mut report);
    metrics::stop();

    // Tray version item (normal + hover), up to date and with an update.
    for (name, latest) in [("menu_version", None), ("menu_update", Some("\u{2191}9.9.9".to_string()))] {
        for dpi in [96, 144] {
            let (w, h, px) = crate::tray::preview(&(format!("Version {}", crate::update::VERSION), latest.clone()), dpi);
            let _ = std::fs::write(dir.join(format!("{name}{dpi}_{w}x{h}.rgba")), px);
        }
    }
    let _ = std::fs::write(dir.join("selftest.txt"), report);
}
