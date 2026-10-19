//! Off-screen self-test: `resource-monitor-rs.exe --selftest <dir>`.
//!
//! Renders frames to raw RGBA files (`<name>_<w>x<h>.rgba`, premultiplied)
//! without opening a window: one from live metrics, the rest from synthetic
//! snapshots that exercise every element (temps, fire, iGPU, game/FPS row,
//! scales, track off) and the tray version item. Also writes timing info to selftest.txt.
use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;

use crate::metrics::{self, Gpu, Snapshot};
use crate::ui::Ui;

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
    }
}

pub fn run(dir: &str) {
    let dir = Path::new(dir);
    let _ = std::fs::create_dir_all(dir);
    let mut report = String::new();
    let info = metrics::info();
    let mut ui = Ui::new(info, 1.0, true);

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

    // Synthetic: every element on, snakes rotated a bit.
    for _ in 0..8 {
        ui.apply(&synth(false));
    }
    for _ in 0..20 {
        ui.step(0.042);
    }
    save(&mut ui, dir, "synth", &mut report);
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
    metrics::stop();

    // Tray version item (normal + hover), up to date and with an update.
    for (name, latest) in [("menu_version", None), ("menu_update", Some("\u{2191}9.9.9".to_string()))] {
        for dpi in [96, 144] {
            let (w, h, px) = crate::tray::preview(&(format!("Version {}", crate::update::VERSION), latest.clone()), dpi);
            let _ = std::fs::write(dir.join(format!("{name}{dpi}_{w}x{h}.rgba")), px);
        }
    }
    links(&mut ui, dir, &mut report);
    let _ = std::fs::write(dir.join("selftest.txt"), report);
}

const RUNPOD_ICON: &str = r##"{"color": "#8b5cf6", "width": 2.2,
 "loops": [[12,2.6, 20.2,7.3, 20.2,16.7, 12,21.4, 3.8,16.7, 3.8,7.3]],
 "lines": [[7.7,9.3, 12,11.8, 16.1,9.4, 16.1,7.5],
           [7.1,12.5, 10.0,14.1, 10.0,17.4],
           [16.9,12.5, 14.0,14.1, 14.0,17.4]]}"##;

/// Linked rows: a running RunPod training (graph), a finished one, a generic progress row.
fn links(ui: &mut Ui, dir: &Path, report: &mut String) {
    let loss = |n: usize, total: f32| -> String {
        let pts: Vec<String> = (1..=n)
            .map(|i| {
                let x = i as f32;
                let v = 0.35 + 2.0 * (-x / (total * 0.12)).exp() + 0.06 * ((x * 1.7).sin() * (x * 0.31).cos());
                format!("[{x},{v:.4}]")
            })
            .collect();
        pts.join(",")
    };
    let train = format!(
        r##"{{"title":"forum-user","icon":{RUNPOD_ICON},"step":1340,"total":3200,"snake":0.93,"eta_s":2830,"caption":"H100 SXM 80GB","metric":"loss","points":[{}]}}"##,
        loss(1340, 3200.0)
    );
    let done = format!(
        r##"{{"title":"bcp-reviews-dpo","icon":{RUNPOD_ICON},"step":800,"total":800,"state":"done","caption":"A100 SXM 80GB","metric":"loss","points":[{}],"colors":["#f472b6","#facc15"]}}"##,
        loss(800, 800.0)
    );
    let generic = r#"{"title":"qwen download","text":"6.1/9.4 GB","progress":0.65}"#.to_string();
    let rows: Vec<_> = [(train, 20), (done, 300), (generic, 1)].iter().filter_map(|(j, age)| crate::links::parse_str(j, *age)).collect();
    let _ = writeln!(report, "links parsed: {}", rows.len());
    for (name, n) in [("links1", 1), ("links3", 3)] {
        let t = Instant::now();
        let mut f = ui.links(&rows[..n], std::time::SystemTime::now()).clone();
        let el = t.elapsed();
        for _ in 0..30 {
            ui.step(0.042);
        }
        let ov = ui.links_overlay().clone();
        f.draw_pixmap(0, 0, ov.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
        for px in f.data_mut().as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
        let _ = std::fs::write(dir.join(format!("{name}_{}x{}.rgba", f.width(), f.height())), f.data());
        let _ = writeln!(report, "{name}: {}x{} render {:?}", f.width(), f.height(), el);
    }
}
