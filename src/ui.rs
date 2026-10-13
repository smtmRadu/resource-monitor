//! Pill UI: layout, 1 Hz static layer, and the animated snakes.
//!
//! Each module is a row: [icon + name capsule] [metrics capsule] plus an
//! optional 60s history ribbon below it. The static layer (capsules, text,
//! gauges, ribbons) is rendered once per data tick into `base`; the rotating
//! snakes live in a separate, narrow `overlay` layer redrawn every frame.
use std::collections::VecDeque;

use tiny_skia::{FillRule, LineCap, Pixmap, PixmapMut, Transform};

use crate::draw::{self, col, col_a, Glyph, MUTED, PILL, RED, TXT, WATT};
use crate::metrics::{Drive, Info, Snapshot, Vendor};
use crate::text::{Anchor, Text};

const PILL_H: f32 = 24.0;
const ICON_W: f32 = 79.0;
const VAL_W: f32 = 167.0;
const MARGIN: f32 = 3.0;
const GAP: f32 = 6.0;
const ROW_H: f32 = 36.0;
const TRACK_H: f32 = 60.0;
const WIDTH: f32 = ICON_W + 2.0 * MARGIN + GAP + VAL_W; // 258
const HIST: usize = 60;
/// Ribbon grid lines and the contours drawn in the same color.
const GRID: u32 = 0x232833;
/// Explorer's "low space" rule: red bar once a drive is over 90% full.
const LOW_SPACE_USED: f32 = 0.90;
const BG_NORMAL: f32 = 1.0;
const BG_GAME: f32 = 0.25; // pill backgrounds fade in game mode; text stays solid

const ROWS: [Glyph; 8] = [Glyph::Cpu, Glyph::Ram, Glyph::Gpu, Glyph::Vram, Glyph::Igpu, Glyph::Disk, Glyph::Net, Glyph::Fps];

fn idx(g: Glyph) -> usize {
    ROWS.iter().position(|&x| x == g).unwrap()
}

fn name(g: Glyph) -> &'static str {
    match g {
        Glyph::Cpu => "CPU",
        Glyph::Ram => "RAM",
        Glyph::Gpu => "GPU",
        Glyph::Vram => "VRAM",
        Glyph::Igpu => "iGPU",
        Glyph::Disk => "DISK",
        Glyph::Net => "ETH",
        Glyph::Fps => "FPS",
    }
}

/// Used share of all fixed drives together (bytes-weighted), 0..1.
fn storage_used(drives: &[Drive]) -> Option<f32> {
    let total: f64 = drives.iter().map(|d| d.total_bytes as f64).sum();
    (total > 0.0).then(|| (drives.iter().map(|d| d.used as f64 * d.total_bytes as f64).sum::<f64>() / total) as f32)
}

fn pct(v: f32) -> String {
    if v < 10.0 {
        format!("{v:.1}%")
    } else {
        format!("{v:.0}%")
    }
}

#[derive(Default, Clone, Copy)]
struct Snake {
    head: f32,
    len: f32,
    target: f32,
}

#[derive(Clone, Copy)]
struct BoxF {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

struct Values {
    cpu_main: String,
    cpu_sub: String,
    cpu_temp: Option<f32>,
    cores: Vec<f32>,
    ram: String,
    gpu_main: String,
    gpu_sub: String,
    gpu_temp: Option<f32>,
    vram: String,
    igpu: String,
    disk: String,
    drives: Vec<Drive>,
    net_down: String,
    net_up: String,
    fps_name: String,
    fps_val: String,
}

pub struct Ui {
    s: f32,
    pub track: bool,
    accent: [u32; 8],
    text: Text,
    game_mode: bool,
    igpu_shown: bool,
    igpu_zero: u32,
    fps_shown: bool,
    v: Values,
    hist: [VecDeque<f32>; 9], // ROWS order + net upload at 8
    snakes: [Snake; 8],
    icon_boxes: Vec<(usize, BoxF)>,
    base: Pixmap,
    overlay: Pixmap,
    dirty: bool,
}

impl Ui {
    pub fn new(info: Info, scale: f32, track: bool) -> Self {
        let cpu_acc = match info.cpu_vendor {
            Vendor::Intel => 0x60a5fa,
            Vendor::Amd => 0xf97316,
            _ => 0x4ade80,
        };
        let gpu_acc = match info.gpu_vendor {
            Vendor::Nvidia => 0x4ade80,
            Vendor::Amd => 0xf97316,
            Vendor::Intel => 0x60a5fa,
            Vendor::Unknown => 0xf472b6,
        };
        let igpu_acc = match info.cpu_vendor {
            Vendor::Intel => 0x60a5fa,
            Vendor::Amd => 0xf97316,
            _ => 0xfb923c,
        };
        let dots = "\u{2026}".to_string();
        Self {
            s: scale,
            track,
            accent: [cpu_acc, 0xeab308, gpu_acc, 0xc084fc, igpu_acc, 0x94a3b8, 0x7dd3fc, 0xfacc15],
            text: Text::new(),
            game_mode: false,
            igpu_shown: false,
            igpu_zero: 999,
            fps_shown: false,
            v: Values {
                cpu_main: dots.clone(),
                cpu_sub: String::new(),
                cpu_temp: None,
                cores: vec![0.0; info.cores],
                ram: dots.clone(),
                gpu_main: dots.clone(),
                gpu_sub: String::new(),
                gpu_temp: None,
                vram: dots.clone(),
                igpu: dots.clone(),
                disk: dots,
                drives: Vec::new(),
                net_down: String::new(),
                net_up: String::new(),
                fps_name: "FPS".into(),
                fps_val: "--".into(),
            },
            hist: Default::default(),
            snakes: [Snake::default(); 8],
            icon_boxes: Vec::new(),
            base: Pixmap::new(1, 1).unwrap(),
            overlay: Pixmap::new(1, 1).unwrap(),
            dirty: true,
        }
    }

    pub fn set_scale(&mut self, s: f32) {
        self.s = s;
        self.dirty = true;
    }

    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    fn u(&self, v: f32) -> f32 {
        v * self.s
    }

    fn font_main(&self) -> i32 {
        (13.333 * self.s).round() as i32
    }

    fn font_num(&self) -> i32 {
        (10.667 * self.s).round() as i32
    }

    fn visible(&self, g: Glyph) -> bool {
        match g {
            Glyph::Igpu => self.igpu_shown,
            Glyph::Fps => self.fps_shown,
            _ => true,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        let rows = ROWS.iter().filter(|&&g| self.visible(g)).count() as f32;
        let per = ROW_H + if self.track { 1.0 + TRACK_H + 4.0 } else { 0.0 };
        (self.u(WIDTH).ceil() as u32, self.u(rows * per).ceil() as u32)
    }

    // ------------------------------------------------------------------ data
    pub fn apply(&mut self, s: &Snapshot) {
        let v = &mut self.v;
        v.cpu_main = pct(s.cpu);
        v.cpu_sub = s.cpu_watts.map(|w| format!("{w:.0}W")).unwrap_or_default();
        v.cpu_temp = s.cpu_temp;
        v.cores.clone_from(&s.cores);
        v.ram = format!("{} {:.1}/{:.0}G", pct(s.ram_pct), s.ram_used_gb, s.ram_total_gb);
        let g = &s.gpu;
        v.gpu_main = g.util.map(pct).unwrap_or_else(|| "--".into());
        v.gpu_sub = match (g.watts, g.limit_w) {
            (Some(w), Some(l)) => format!("{w:.0}/{l:.0}W"),
            (Some(w), None) => format!("{w:.0}W"),
            _ => String::new(),
        };
        v.gpu_temp = g.temp;
        let vpct = match (g.vram_used_mb, g.vram_total_mb) {
            (Some(u), Some(t)) if t > 0.0 => Some((u / t * 100.0, u, t)),
            _ => None,
        };
        v.vram = vpct
            .map(|(p, u, t)| format!("{} {:.1}/{:.0}G", pct(p), u / 1024.0, t / 1024.0))
            .unwrap_or_else(|| "--".into());
        // iGPU row: visible on activity, hidden after 5s at 0%.
        match s.igpu {
            Some(ig) if ig > 0.0 => {
                self.igpu_zero = 0;
                self.igpu_shown = true;
                v.igpu = pct(ig);
            }
            _ => {
                v.igpu = "0.0%".into();
                self.igpu_zero = self.igpu_zero.saturating_add(1);
                if self.igpu_zero >= 5 {
                    self.igpu_shown = false;
                }
            }
        }
        v.disk = format!("{:.1}MB/s", s.disk_mbs);
        v.drives.clone_from(&s.drives);
        v.net_down = format!("{:.1}", s.net_down);
        v.net_up = format!("{:.1}", s.net_up);
        // FPS row only exists while a game runs.
        let hz = s.hz.max(1) as f32;
        match &s.game {
            Some(game) => {
                let disp = game.strip_suffix(".exe").or_else(|| game.strip_suffix(".EXE")).unwrap_or(game);
                v.fps_name = disp.chars().take(12).collect();
                v.fps_val = s.fps.map(|f| format!("{f:.0} FPS")).unwrap_or_else(|| "--".into());
                self.fps_shown = true;
            }
            None => {
                if self.fps_shown {
                    self.hist[idx(Glyph::Fps)].clear(); // next game starts a fresh graph
                }
                self.fps_shown = false;
            }
        }
        self.game_mode = s.game.is_some();

        let vram_p = vpct.map(|x| x.0);
        let targets = [
            Some(s.cpu),
            Some(s.ram_pct),
            Some(g.util.unwrap_or(0.0)),
            vram_p.or(Some(0.0)),
            Some(s.igpu.unwrap_or(0.0)),
            storage_used(&s.drives).map(|u| u * 100.0),
            None,
            Some(s.fps.map(|f| f / hz * 100.0).unwrap_or(0.0)),
        ];
        for (i, t) in targets.iter().enumerate() {
            if let Some(t) = t {
                self.snakes[i].target = (t / 100.0).clamp(0.0, 1.0);
            }
        }
        let mut push = |i: usize, x: Option<f32>| {
            if let Some(x) = x {
                let h = &mut self.hist[i];
                if h.len() >= HIST {
                    h.pop_front();
                }
                h.push_back(x.clamp(0.0, 100.0));
            }
        };
        push(0, Some(s.cpu));
        push(1, Some(s.ram_pct));
        push(2, Some(g.util.unwrap_or(0.0)));
        push(3, vram_p);
        push(4, s.igpu);
        push(5, Some(s.disk_mbs * 0.2)); // 500 MB/s = full scale
        push(6, Some(s.net_down)); // 100 MB/s = full scale
        push(8, Some(s.net_up));
        push(7, s.fps.map(|f| f / hz * 100.0));
        self.dirty = true;
    }

    // ------------------------------------------------------------- rendering
    fn render_base(&mut self) {
        let (w, h) = self.size();
        if self.base.width() != w || self.base.height() != h {
            self.base = Pixmap::new(w.max(1), h.max(1)).unwrap();
        } else {
            self.base.fill(tiny_skia::Color::TRANSPARENT);
        }
        let mut base = std::mem::replace(&mut self.base, Pixmap::new(1, 1).unwrap());
        self.icon_boxes.clear();
        let mut y = 0.0f32;
        for g in ROWS {
            if !self.visible(g) {
                continue;
            }
            self.render_row(&mut base.as_mut(), g, y);
            y += self.u(ROW_H);
            if self.track {
                let tb = BoxF { x: 0.0, y: (y + self.u(1.0)).round(), w: self.u(WIDTH).round(), h: self.u(TRACK_H).round() };
                self.render_track(&mut base.as_mut(), g, tb);
                y += self.u(1.0 + TRACK_H + 4.0);
            }
        }
        self.base = base;
        self.dirty = false;
    }

    fn bg(&self) -> f32 {
        if self.game_mode {
            BG_GAME
        } else {
            BG_NORMAL
        }
    }

    fn capsule(&self, pix: &mut PixmapMut, b: BoxF) {
        let m = self.u(2.0);
        let hh = b.h - 2.0 * m;
        draw::fill_rrect(pix, b.x + m, b.y + m, b.w - 2.0 * m, hh, hh / 2.0, col_a(PILL, self.bg()));
    }

    /// Grid-colored contour (same color/alpha as the ribbons' 50% line), inset
    /// half a line inside the shape at (x, y, w, h) with corner radius `r`.
    fn contour(&self, pix: &mut PixmapMut, x: f32, y: f32, w: f32, h: f32, r: f32) {
        let lw = self.s.round().max(1.0);
        let k = lw / 2.0;
        if let Some(p) = draw::rrect(x + k, y + k, w - lw, h - lw, r - k) {
            let c = col_a(GRID, self.bg());
            pix.stroke_path(&p, &draw::paint(c), &draw::stroke(lw, LineCap::Butt), Transform::identity(), None);
        }
    }

    fn render_row(&mut self, pix: &mut PixmapMut, g: Glyph, y: f32) {
        let s = self.s;
        let fm = self.font_main();
        let i = idx(g);
        let ph = self.u(PILL_H).round();
        let (iw, label) = if g == Glyph::Fps {
            let n = self.v.fps_name.clone();
            let w = self.u(34.0 + 12.0) + self.text.measure(fm, &n) as f32;
            (w.min(self.u(252.0 - GAP - 60.0)).round(), n)
        } else {
            (self.u(ICON_W).round(), name(g).to_string())
        };
        let vw = if g == Glyph::Fps {
            (self.u(252.0 - GAP) - iw).max(self.u(60.0)).round()
        } else {
            self.u(VAL_W).round()
        };
        let top = (y + self.u(6.0)).round();
        let ib = BoxF { x: self.u(MARGIN).round(), y: top, w: iw, h: ph };
        let vb = BoxF { x: (iw + self.u(2.0 * MARGIN + GAP)).round(), y: top, w: vw, h: ph };
        let ty = top + (ph / 2.0).floor() + 1.0;

        // [icon + name]
        self.capsule(pix, ib);
        let m = self.u(2.0);
        let hh = ib.h - 2.0 * m;
        self.contour(pix, ib.x + m, ib.y + m, ib.w - 2.0 * m, hh, hh / 2.0);
        draw::icon(pix, g, ib.x, ib.y - self.u(3.0), s, col(self.accent[i]));
        self.label(pix, fm, &label, ib.x + self.u(34.0), ty, TXT, Anchor::West);
        self.icon_boxes.push((i, ib));

        // [metrics]
        self.capsule(pix, vb);
        self.contour(pix, vb.x + m, vb.y + m, vb.w - 2.0 * m, hh, hh / 2.0);
        let tx = vb.x + self.u(12.0);
        match g {
            Glyph::Cpu => {
                let (m, sub) = (self.v.cpu_main.clone(), self.v.cpu_sub.clone());
                self.split_text(pix, &m, &sub, tx, ty);
                let label = if sub.is_empty() { format!("{m} ") } else { format!("{m} {sub}") };
                let end = self.u(12.0) + self.text.measure(fm, &label) as f32;
                self.minis(pix, vb, end);
                self.gauge(pix, vb, self.v.cpu_temp);
            }
            Glyph::Gpu => {
                let (m, sub) = (self.v.gpu_main.clone(), self.v.gpu_sub.clone());
                self.split_text(pix, &m, &sub, tx, ty);
                self.gauge(pix, vb, self.v.gpu_temp);
            }
            Glyph::Net => self.eth(pix, vb),
            Glyph::Fps => {
                let t = self.v.fps_val.clone();
                self.label(pix, fm, &t, vb.x + vb.w / 2.0, ty, TXT, Anchor::Center);
            }
            _ => {
                let t = match g {
                    Glyph::Ram => &self.v.ram,
                    Glyph::Vram => &self.v.vram,
                    Glyph::Igpu => &self.v.igpu,
                    _ => &self.v.disk,
                }
                .clone();
                self.label(pix, fm, &t, tx, ty, TXT, Anchor::West);
                if g == Glyph::Disk {
                    self.drive_bars(pix, vb);
                }
            }
        }
    }

    /// Storage bars right of the DISK speed: one elongated pill per fixed drive,
    /// letter on the left, colored by physical disk (1st blue, 2nd yellow, ...).
    /// At most 2 rows (so letters stay readable); 3+ drives flow into columns.
    /// Starts at a fixed x (widest speed text) so bars don't jump as the number changes.
    /// A drive turns red when under 10% is free: the same rule File Explorer uses
    /// for its red capacity bar in "This PC".
    fn drive_bars(&mut self, pix: &mut PixmapMut, b: BoxF) {
        const DISK_COLORS: [u32; 4] = [0x60a5fa, 0xfacc15, 0x4ade80, 0xf472b6];
        const MAX_DRIVES: usize = 6;
        let drives: Vec<Drive> = self.v.drives.iter().take(MAX_DRIVES).cloned().collect();
        if drives.is_empty() {
            return;
        }
        let mut disks: Vec<u32> = drives.iter().map(|d| d.disk).collect();
        disks.sort_unstable();
        disks.dedup();
        let rows = drives.len().min(2);
        let cols = drives.len().div_ceil(2);
        let fm = self.font_main();
        let x0 = b.x + self.u(12.0 + 8.0) + self.text.measure(fm, "999.9MB/s") as f32;
        let x1 = b.x + b.w - self.u(10.0);
        let gap = self.u(5.0);
        let cell = (x1 - x0 - gap * (cols as f32 - 1.0)) / cols as f32;
        let (top, inner) = (b.y + self.u(4.0), self.u(16.0));
        let pitch = inner / rows as f32;
        let bh = (pitch - self.u(2.5)).clamp(self.u(3.0), self.u(5.0));
        let fpx = ((pitch * 1.3).min(self.u(11.0)).round() as i32).max(7);
        let lw = self.text.measure(fpx, "C:") as f32 + self.u(3.0);
        let bw = cell - lw;
        if bw < self.u(8.0) {
            return;
        }
        for (i, d) in drives.iter().enumerate() {
            let (c, r) = (i / rows, i % rows); // column-major: C: D: | E: F:
            let cx = x0 + c as f32 * (cell + gap);
            let cy = top + pitch * (r as f32 + 0.5);
            self.label(pix, fpx, &format!("{}:", d.letter), cx, cy, MUTED, Anchor::West);
            let (bx, y) = (cx + lw, cy - bh / 2.0);
            draw::fill_rrect(pix, bx, y, bw, bh, bh / 2.0, col(0x2a303c));
            let k = if d.used >= LOW_SPACE_USED {
                RED
            } else {
                DISK_COLORS[disks.iter().position(|&k| k == d.disk).unwrap_or(0) % DISK_COLORS.len()]
            };
            draw::fill_rrect(pix, bx, y, (bw * d.used.clamp(0.0, 1.0)).max(bh), bh, bh / 2.0, col(k));
        }
    }

    /// Text; in game mode the pills are faded, so add a dark halo to stay readable.
    #[allow(clippy::too_many_arguments)]
    fn label(&mut self, pix: &mut PixmapMut, px: i32, s: &str, x: f32, y: f32, c: u32, a: Anchor) {
        if self.game_mode {
            let d = self.s.round().max(1.0);
            for (dx, dy) in [(-d, 0.0), (d, 0.0), (0.0, -d), (0.0, d)] {
                self.text.draw(pix, px, s, x + dx, y + dy, 0x000000, a);
            }
        }
        self.text.draw(pix, px, s, x, y, c, a);
    }

    /// White value + lightning-yellow wattage right after it.
    fn split_text(&mut self, pix: &mut PixmapMut, main: &str, sub: &str, x: f32, y: f32) {
        let fm = self.font_main();
        self.label(pix, fm, main, x, y, TXT, Anchor::West);
        if !sub.is_empty() {
            let dx = self.text.measure(fm, &format!("{main} ")) as f32;
            self.label(pix, fm, sub, x + dx, y, WATT, Anchor::West);
        }
    }

    /// Per-core micro grid, centered between the text end and the temp ring.
    fn minis(&self, pix: &mut PixmapMut, b: BoxF, text_end: f32) {
        let n = self.v.cores.len();
        if n == 0 {
            return;
        }
        let (cols, rows) = if n <= 8 {
            (n.div_ceil(2), 2)
        } else if n <= 32 {
            (8, n.div_ceil(8))
        } else {
            (16, n.div_ceil(16))
        };
        let step = self.u(4.0).round().max(4.0);
        let sq = step - (self.s.round().max(1.0));
        let strip = cols as f32 * step - (step - sq);
        let gauge_x = b.w - b.h;
        let ox = (b.x + text_end + ((gauge_x - text_end - strip) / 2.0).max(self.u(4.0))).round();
        let oy = (b.y + ((b.h - (rows as f32 * step - (step - sq))) / 2.0).ceil()).round();
        for (i, &v) in self.v.cores.iter().enumerate() {
            let (r, c) = (i / cols, i % cols);
            draw::fill_rect(pix, ox + c as f32 * step, oy + r as f32 * step, sq, sq, col(draw::usage_color(v)));
        }
    }

    /// Temperature ring on the capsule's right end disc; fire glyph above 99°.
    fn gauge(&mut self, pix: &mut PixmapMut, b: BoxF, temp: Option<f32>) {
        let Some(raw) = temp else { return };
        let (cx, cy) = (b.x + b.w - b.h / 2.0, b.y + b.h / 2.0);
        let r = self.u(8.9);
        let w = self.u(1.6);
        if let Some(p) = draw::arc_path(cx, cy, r, r, 0.0, 360.0) {
            pix.stroke_path(&p, &draw::paint(col(0x39404e)), &draw::stroke(w, LineCap::Butt), Transform::identity(), None);
        }
        let frac = ((raw - 40.0) / 59.0).clamp(0.0, 1.0);
        if frac > 0.0 {
            // from 12 o'clock, clockwise
            if let Some(p) = draw::arc_path(cx, cy, r, r, 90.0, -frac * 360.0) {
                let c = col(draw::usage_color(frac * 100.0));
                pix.stroke_path(&p, &draw::paint(c), &draw::stroke(w, LineCap::Round), Transform::identity(), None);
            }
        }
        if raw > 99.0 {
            let s = self.s;
            let flame = [(-4.0, 5.0), (-4.2, 1.0), (-2.5, -1.0), (-3.2, -3.5), (-0.5, -6.0), (1.5, -2.5), (4.0, -0.5), (3.5, 5.0)];
            let ember = [(-1.5, 4.0), (-1.8, 1.0), (0.0, -1.5), (1.8, 1.5), (1.5, 4.0)];
            for (pts, c) in [(&flame[..], RED), (&ember[..], 0xfacc15)] {
                let v: Vec<(f32, f32)> = pts.iter().map(|&(x, y)| (cx + x * s, cy + y * s)).collect();
                if let Some(p) = draw::polyline(&v, true) {
                    pix.fill_path(&p, &draw::paint(col(c)), FillRule::Winding, Transform::identity(), None);
                }
            }
        } else {
            let t = format!("{}", raw.round().clamp(0.0, 99.0) as i32);
            let fnum = self.font_num();
            self.label(pix, fnum, &t, cx, cy - 1.0, TXT, Anchor::Center);
        }
    }

    /// ETH capsule: drawn down/up arrows with their values, then "MB/s".
    fn eth(&mut self, pix: &mut PixmapMut, b: BoxF) {
        let s = self.s;
        let fm = self.font_main();
        let y = b.y + b.h / 2.0;
        let (dx, ux) = (b.x + (10.0 * s).floor(), b.x + (64.0 * s).floor());
        let down = col(self.accent[idx(Glyph::Net)]);
        let up = col(0xc084fc);
        draw::fill_rect(pix, dx + 2.5 * s, y - 8.0 * s, 2.0 * s, 7.0 * s, down);
        let tri = |x: f32, base: f32, tip: f32| [(x, base), (x + 7.0 * s, base), (x + 3.5 * s, tip)];
        if let Some(p) = draw::polyline(&tri(dx, y - 1.0 * s, y + 5.0 * s), true) {
            pix.fill_path(&p, &draw::paint(down), FillRule::Winding, Transform::identity(), None);
        }
        draw::fill_rect(pix, ux + 2.5 * s, y + 1.0 * s, 2.0 * s, 7.0 * s, up);
        if let Some(p) = draw::polyline(&tri(ux, y + 1.0 * s, y - 5.0 * s), true) {
            pix.fill_path(&p, &draw::paint(up), FillRule::Winding, Transform::identity(), None);
        }
        let ty = b.y + (b.h / 2.0).floor() + 1.0;
        let (d, u) = (self.v.net_down.clone(), self.v.net_up.clone());
        self.label(pix, fm, &d, dx + 10.0 * s, ty, TXT, Anchor::West);
        self.label(pix, fm, &u, ux + 10.0 * s, ty, TXT, Anchor::West);
        self.label(pix, fm, "MB/s", b.x + (118.0 * s).floor(), ty, MUTED, Anchor::West);
    }

    /// 60s history ribbon.
    fn render_track(&self, pix: &mut PixmapMut, g: Glyph, b: BoxF) {
        let m = self.u(2.0);
        draw::fill_rrect(pix, b.x + m, b.y + m, b.w - 2.0 * m, b.h - 2.0 * m, self.u(PILL_H / 2.0), col_a(PILL, self.bg()));
        let gy = |p: f32| b.y + b.h - self.u(5.0) - p / 100.0 * (b.h - self.u(10.0));
        let (x0, x1) = (b.x + self.u(6.0), b.x + b.w - self.u(6.0));
        let grid = col_a(GRID, self.bg());
        let lw = self.s.round().max(1.0);
        self.contour(pix, b.x + m, b.y + m, b.w - 2.0 * m, b.h - 2.0 * m, self.u(PILL_H / 2.0));
        draw::fill_rect(pix, x0, gy(50.0).round(), x1 - x0, lw, grid);
        for p in [25.0, 75.0] {
            let mut x = x0;
            while x < x1 {
                draw::fill_rect(pix, x, gy(p).round(), (self.u(8.0)).min(x1 - x), lw, grid);
                x += self.u(16.0);
            }
        }
        let line = |pix: &mut PixmapMut, vals: &VecDeque<f32>, c: u32| {
            if vals.len() < 2 {
                return;
            }
            let n = vals.len() as f32 - 1.0;
            let pts: Vec<(f32, f32)> = vals.iter().enumerate().map(|(i, &v)| (x0 + i as f32 * (x1 - x0) / n, gy(v))).collect();
            if let Some(p) = draw::polyline(&pts, false) {
                pix.stroke_path(&p, &draw::paint(col(c)), &draw::stroke(self.u(2.0), LineCap::Round), Transform::identity(), None);
            }
        };
        let i = idx(g);
        line(pix, &self.hist[i], self.accent[i]);
        if g == Glyph::Net {
            line(pix, &self.hist[8], 0xc084fc);
        }
    }

    // ------------------------------------------------------------ animation
    /// Advance snakes by `dt` seconds (tuned to the original 0.009/frame at 24 fps).
    pub fn step(&mut self, dt: f32) {
        let f = dt / 0.042;
        let ease = 1.0 - 0.88f32.powf(f);
        for s in &mut self.snakes {
            s.head = (s.head + 0.009 * f).fract();
            s.len += (s.target - s.len) * ease;
        }
    }

    /// True while some visible snake moves (rotating segment or easing length).
    pub fn animating(&self) -> bool {
        self.icon_boxes.iter().any(|&(i, _)| {
            let s = self.snakes[i];
            let full = s.len > 0.985 && s.target > 0.985;
            (s.len >= 0.01 && !full) || (s.target - s.len).abs() > 0.002
        })
    }

    /// Static layer (capsules, text, gauges, ribbons), re-rendered only when dirty.
    pub fn base(&mut self) -> &Pixmap {
        if self.dirty {
            self.render_base();
        }
        &self.base
    }

    fn snake_rows(&self) -> impl Iterator<Item = (usize, BoxF)> + '_ {
        self.icon_boxes.iter().copied().filter(|&(i, _)| ROWS[i] != Glyph::Net)
    }

    /// Snake layer: transparent except the snakes; spans only the icon column
    /// down to the last snake row. Drawn in BGRA order (ready for the window DIB).
    pub fn overlay(&mut self) -> &Pixmap {
        if self.dirty {
            self.render_base(); // layout (icon boxes) must be current
        }
        let pad = self.u(2.0).ceil() + 1.0;
        let w = self.snake_rows().map(|(_, b)| b.x + b.w + pad).fold(1.0, f32::max).ceil() as u32;
        let h = (self.snake_rows().map(|(_, b)| b.y + b.h + pad).fold(1.0, f32::max).ceil() as u32).min(self.base.height());
        if self.overlay.width() != w || self.overlay.height() != h {
            self.overlay = Pixmap::new(w.max(1), h.max(1)).unwrap();
        } else {
            self.overlay.fill(tiny_skia::Color::TRANSPARENT);
        }
        let mut ov = std::mem::replace(&mut self.overlay, Pixmap::new(1, 1).unwrap());
        let rows: Vec<(usize, BoxF)> = self.snake_rows().collect();
        for (i, b) in rows {
            self.snake(&mut ov.as_mut(), i, b);
        }
        self.overlay = ov;
        &self.overlay
    }

    fn snake(&self, pix: &mut PixmapMut, i: usize, b: BoxF) {
        let sn = self.snakes[i];
        if sn.len < 0.01 {
            return;
        }
        let pad = self.u(1.0);
        let r = b.h / 2.0 - pad;
        let (cx0, cx1, cy) = (b.x + b.h / 2.0, b.x + b.w - b.h / 2.0, b.y + b.h / 2.0);
        let straight = b.w - b.h;
        let pi = std::f32::consts::PI;
        let total = 2.0 * straight + 2.0 * pi * r;
        // perimeter point, starting top-left going clockwise
        let at = |d: f32| -> (f32, f32) {
            let d = d.rem_euclid(total);
            if d < straight {
                (cx0 + d, b.y + pad)
            } else if d < straight + pi * r {
                let a = (d - straight) / r - pi / 2.0;
                (cx1 + r * a.cos(), cy + r * a.sin())
            } else if d < 2.0 * straight + pi * r {
                (cx1 - (d - straight - pi * r), b.y + b.h - pad)
            } else {
                let a = (d - 2.0 * straight - pi * r) / r + pi / 2.0;
                (cx0 + r * a.cos(), cy + r * a.sin())
            }
        };
        let full = sn.len > 0.985 && sn.target > 0.985;
        let (start, len) = if full { (0.0, total) } else { (sn.head * total - sn.len.max(6.0 / 280.0) * total, sn.len.max(6.0 / 280.0) * total) };
        let n = (len / self.u(3.0)).ceil().max(2.0) as usize;
        let pts: Vec<(f32, f32)> = (0..=n).map(|k| at(start + len * k as f32 / n as f32)).collect();
        if let Some(p) = draw::polyline(&pts, full) {
            let c = self.accent[i];
            let bgr = ((c & 0xff) << 16) | (c & 0xff00) | ((c >> 16) & 0xff);
            pix.stroke_path(&p, &draw::paint(col(bgr)), &draw::stroke(self.u(2.0), LineCap::Butt), Transform::identity(), None);
        }
    }
}
