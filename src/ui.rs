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
const DEV_H: f32 = 20.0;
const ROW_ANIM_S: f32 = 0.3; // row collapse / expand duration
const SAMPLE_S: f32 = 1.0; // metrics period: one ribbon step
const FADE_S: f32 = 0.3; // old -> new values crossfade after each sample
const LINES_FPS: f32 = 30.0; // ribbon scroll redraw rate (slow motion; more buys nothing) // "developer mode" line under the rows (dev builds)
/// Ribbon grid lines and the contours drawn in the same color.
const GRID: u32 = 0x232833;
const TRACK_LBL: u32 = 0x5c6370; // ribbon captions (model names, Hz): quieter than MUTED
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
    /// FPS above the game monitor's refresh rate (not v-synced): yellow value.
    fps_over: bool,
    /// Game monitor refresh rate: the FPS ribbon's full scale.
    fps_hz: u32,
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
    cpu_name: String,
    gpu_name: String,
    /// Per row: 0 = collapsed, 1 = fully shown; eases toward `visible` (linear progress).
    open: [f32; 8],
    icon_boxes: Vec<(usize, BoxF)>,
    /// Everything but the ribbon lines; re-rendered only when dirty.
    stat: Pixmap,
    /// The previous `stat`, faded out over FADE_S after a new sample.
    prev: Pixmap,
    /// Crossfade progress 0..1 (1: done); `fade_next`: the next render starts one.
    fade: f32,
    fade_next: bool,
    /// `stat` + ribbon lines at the current scroll phase (what the window shows).
    base: Pixmap,
    /// Ribbons whose lines are drawn per frame: (row index, track box).
    track_boxes: Vec<(usize, BoxF)>,
    /// Temperature rings drawn per frame: (0 CPU / 1 GPU, value pill box).
    gauge_boxes: Vec<(usize, BoxF)>,
    /// Shown CPU / GPU temperatures, easing toward the sampled ones.
    temps: [Option<f32>; 2],
    /// Rendering an off-screen block (collapsing row): draw gauges in place.
    baking: bool,
    /// 0..1 progress from the last sample toward the next: lines scroll by it.
    phase: f32,
    line_acc: f32,
    lines_dirty: bool,
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
                fps_over: false,
                fps_hz: 0,
            },
            hist: Default::default(),
            snakes: [Snake::default(); 8],
            cpu_name: info.cpu_name.clone(),
            gpu_name: info.gpu_name.clone(),
            open: [1.0, 1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 0.0], // iGPU and FPS start hidden
            icon_boxes: Vec::new(),
            stat: Pixmap::new(1, 1).unwrap(),
            prev: Pixmap::new(1, 1).unwrap(),
            fade: 1.0,
            fade_next: false,
            base: Pixmap::new(1, 1).unwrap(),
            track_boxes: Vec::new(),
            gauge_boxes: Vec::new(),
            temps: [None, None],
            baking: false,
            phase: 1.0,
            line_acc: 0.0,
            lines_dirty: true,
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

    /// Row + track height (unscaled).
    fn per(&self) -> f32 {
        ROW_H + if self.track { 1.0 + TRACK_H + 4.0 } else { 0.0 }
    }

    /// Eased share of a row's height currently shown (0..1).
    fn shown(&self, i: usize) -> f32 {
        let p = self.open[i];
        p * p * (3.0 - 2.0 * p)
    }

    pub fn size(&self) -> (u32, u32) {
        let rows: f32 = (0..ROWS.len()).map(|i| self.shown(i)).sum();
        let dev = if crate::log::dev() { DEV_H } else { 0.0 };
        (self.u(WIDTH).ceil() as u32, self.u(rows * self.per() + dev).ceil() as u32)
    }

    /// Ribbon lines still scrolling toward the newest sample.
    fn lines_moving(&self) -> bool {
        (self.phase < 1.0 && !self.track_boxes.is_empty()) || self.fade < 1.0 || self.gauge_moving(0) || self.gauge_moving(1)
    }

    /// A ribbon frame is due (base layer must be pushed this frame).
    pub fn lines_due(&self) -> bool {
        self.lines_dirty
    }

    /// True while a row collapses or expands (the base layer must re-render each frame).
    pub fn rows_moving(&self) -> bool {
        ROWS.iter().enumerate().any(|(i, &g)| self.open[i] != if self.visible(g) { 1.0 } else { 0.0 })
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
                // 2% slack: a v-synced game jitters just above the refresh rate.
                v.fps_over = s.fps.is_some_and(|f| f > hz * 1.02);
                v.fps_hz = s.hz;
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
                // One sample past the left edge: the scrolling tail slides out instead of popping.
                if h.len() > HIST {
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
        self.phase = 0.0;
        self.fade_next = true;
        self.dirty = true;
    }

    // ------------------------------------------------------------- rendering
    fn render_base(&mut self) {
        let (w, h) = self.size();
        // New sample, same layout: keep the old frame to crossfade from.
        self.fade = 1.0;
        if std::mem::take(&mut self.fade_next) && self.stat.width() == w && self.stat.height() == h {
            std::mem::swap(&mut self.stat, &mut self.prev);
            self.fade = 0.0;
        }
        if self.stat.width() != w || self.stat.height() != h {
            self.stat = Pixmap::new(w.max(1), h.max(1)).unwrap();
        } else {
            self.stat.fill(tiny_skia::Color::TRANSPARENT);
        }
        let mut base = std::mem::replace(&mut self.stat, Pixmap::new(1, 1).unwrap());
        self.icon_boxes.clear();
        self.track_boxes.clear();
        self.gauge_boxes.clear();
        let mut y = 0.0f32;
        for (i, g) in ROWS.into_iter().enumerate() {
            let k = self.shown(i);
            if k <= 0.0 {
                continue;
            }
            if k >= 1.0 {
                if let Some(tb) = self.render_row_block(&mut base.as_mut(), g, y, false) {
                    self.track_boxes.push((i, tb));
                }
            } else {
                // Collapsing / expanding: the full block, cut to its current height
                // from the top and faded; no snake while it moves.
                let full = self.u(self.per()).ceil() as u32;
                let mut tmp = Pixmap::new(w.max(1), full.max(1)).unwrap();
                let n = self.icon_boxes.len();
                self.baking = true;
                self.render_row_block(&mut tmp.as_mut(), g, 0.0, true);
                self.baking = false;
                self.icon_boxes.truncate(n);
                let ch = ((full as f32 * k).round() as u32).max(1);
                let row = 4 * w as usize;
                let mut cut = Pixmap::new(w.max(1), ch).unwrap();
                cut.data_mut().copy_from_slice(&tmp.data()[..ch as usize * row]);
                let paint = tiny_skia::PixmapPaint { opacity: k, ..Default::default() };
                base.draw_pixmap(0, y.round() as i32, cut.as_ref(), &paint, Transform::identity(), None);
            }
            y += self.u(self.per()) * k;
        }
        if crate::log::dev() {
            let (fm, t) = (self.font_num(), "developer mode");
            let w = self.text.measure(fm, t) as f32 + self.u(16.0);
            let b = BoxF { x: ((self.u(WIDTH) - w) / 2.0).round(), y: y.round(), w: w.round(), h: self.u(DEV_H).round() };
            self.capsule(&mut base.as_mut(), b);
            self.label(&mut base.as_mut(), fm, t, b.x + b.w / 2.0, b.y + b.h / 2.0, WATT, Anchor::Center);
        }
        self.stat = base;
        self.dirty = false;
        self.lines_dirty = true;
    }

    /// `base` = `stat` + ribbon lines and captions at the current scroll phase.
    fn compose(&mut self) {
        if self.base.width() != self.stat.width() || self.base.height() != self.stat.height() {
            self.base = self.stat.clone();
        } else {
            self.base.data_mut().copy_from_slice(self.stat.data());
        }
        if self.fade < 1.0 && self.prev.width() == self.stat.width() && self.prev.height() == self.stat.height() {
            // Premultiplied lerp prev -> stat; only changed pixels (values) actually move.
            let t = self.fade * self.fade * (3.0 - 2.0 * self.fade);
            let k = (t * 256.0) as u32;
            for (d, &o) in self.base.data_mut().iter_mut().zip(self.prev.data()) {
                let n = *d as u32;
                if n != o as u32 {
                    *d = ((o as u32 * (256 - k) + n * k) >> 8) as u8;
                }
            }
        }
        let mut base = std::mem::replace(&mut self.base, Pixmap::new(1, 1).unwrap());
        for (i, tb) in self.track_boxes.clone() {
            self.track_lines(&mut base.as_mut(), ROWS[i], tb);
        }
        for (k, b) in self.gauge_boxes.clone() {
            // The ring's square comes straight from `stat` (no crossfade ghost of the old number).
            let (bw, x0) = (self.stat.width() as usize, (b.x + b.w - b.h).max(0.0) as usize);
            let x1 = ((b.x + b.w).ceil() as usize).min(bw);
            let (y0, y1) = (b.y.max(0.0) as usize, ((b.y + b.h).ceil() as usize).min(self.stat.height() as usize));
            for y in y0..y1 {
                let r = (y * bw + x0) * 4..(y * bw + x1) * 4;
                base.data_mut()[r.clone()].copy_from_slice(&self.stat.data()[r]);
            }
            self.gauge(&mut base.as_mut(), b, self.temps[k]);
        }
        self.base = base;
        self.lines_dirty = false;
    }

    /// One row (pills) plus its track at `y`; returns the track box. `lines`: also
    /// draw the ribbon lines now (else `compose` draws them per frame).
    fn render_row_block(&mut self, pix: &mut PixmapMut, g: Glyph, y: f32, lines: bool) -> Option<BoxF> {
        self.render_row(pix, g, y);
        if !self.track {
            return None;
        }
        let ty = y + self.u(ROW_H);
        let tb = BoxF { x: 0.0, y: (ty + self.u(1.0)).round(), w: self.u(WIDTH).round(), h: self.u(TRACK_H).round() };
        self.render_track(pix, tb);
        if lines {
            self.track_lines(pix, g, tb);
        }
        Some(tb)
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
        let ty = top + ph / 2.0;

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
                self.gauge_slot(pix, vb, 0);
            }
            Glyph::Gpu => {
                let (m, sub) = (self.v.gpu_main.clone(), self.v.gpu_sub.clone());
                self.split_text(pix, &m, &sub, tx, ty);
                self.gauge_slot(pix, vb, 1);
            }
            Glyph::Net => self.eth(pix, vb),
            Glyph::Fps => {
                let t = self.v.fps_val.clone();
                let c = if self.v.fps_over { WATT } else { TXT };
                self.label(pix, fm, &t, vb.x + vb.w / 2.0, ty, c, Anchor::Center);
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
    /// Temperature ring: drawn per frame by `compose` while it eases, else baked in place.
    fn gauge_slot(&mut self, pix: &mut PixmapMut, b: BoxF, k: usize) {
        if self.baking || !self.gauge_moving(k) {
            self.gauge(pix, b, self.temps[k]);
        } else {
            self.gauge_boxes.push((k, b));
        }
    }

    /// Shown temperature `k` still easing toward the sampled one.
    fn gauge_moving(&self, k: usize) -> bool {
        let target = if k == 0 { self.v.cpu_temp } else { self.v.gpu_temp };
        match (target, self.temps[k]) {
            (Some(g), Some(t)) => (g - t).abs() > 0.05,
            (g, t) => g.is_some() != t.is_some(),
        }
    }

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
            self.label(pix, fnum, &t, cx, cy, TXT, Anchor::Center);
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
        // Stroked arrow (shaft + chevron), as tall as the digits next to it.
        let arrow = |pix: &mut PixmapMut, x: f32, dir: f32, c| {
            let (h, w) = (5.0 * s, 3.5 * s); // half height, chevron half width
            let (tail, tip) = (y - dir * h, y + dir * h);
            let mut pb = tiny_skia::PathBuilder::new();
            pb.move_to(x, tail);
            pb.line_to(x, tip);
            pb.move_to(x - w, tip - dir * w);
            pb.line_to(x, tip);
            pb.line_to(x + w, tip - dir * w);
            if let Some(p) = pb.finish() {
                pix.stroke_path(&p, &draw::paint(c), &draw::stroke(1.8 * s, LineCap::Round), Transform::identity(), None);
            }
        };
        arrow(pix, dx + 3.5 * s, 1.0, down);
        arrow(pix, ux + 3.5 * s, -1.0, up);
        let ty = y;
        let (d, u) = (self.v.net_down.clone(), self.v.net_up.clone());
        self.label(pix, fm, &d, dx + 10.0 * s, ty, TXT, Anchor::West);
        self.label(pix, fm, &u, ux + 10.0 * s, ty, TXT, Anchor::West);
        self.label(pix, fm, "MB/s", b.x + (118.0 * s).floor(), ty, MUTED, Anchor::West);
    }

    /// 60s history ribbon.
    fn render_track(&mut self, pix: &mut PixmapMut, b: BoxF) {
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
    }

    /// Ribbon lines + captions. Lines scroll left by `phase` of a step and end in
    /// a head easing from the previous sample to the newest (one sample behind,
    /// so the motion never jumps).
    fn track_lines(&mut self, pix: &mut PixmapMut, g: Glyph, b: BoxF) {
        let gy = |p: f32| b.y + b.h - self.u(5.0) - p / 100.0 * (b.h - self.u(10.0));
        let (x0, x1) = (b.x + self.u(6.0), b.x + b.w - self.u(6.0));
        let (step, ph) = ((x1 - x0) / (HIST as f32 - 1.0), self.phase);
        let line = |pix: &mut PixmapMut, vals: &VecDeque<f32>, c: u32| {
            let n = vals.len();
            if n < 2 {
                return;
            }
            let mut pts: Vec<(f32, f32)> =
                vals.iter().take(n - 1).enumerate().map(|(i, &v)| (x1 - ((n - 2 - i) as f32 + ph) * step, gy(v))).collect();
            pts.push((x1, gy(vals[n - 2] + (vals[n - 1] - vals[n - 2]) * ph)));
            // Clip the tail exactly at the left edge.
            if let Some(k) = pts.iter().position(|p| p.0 >= x0) {
                if k > 0 {
                    let ((ax, ay), (bx, by)) = (pts[k - 1], pts[k]);
                    let t = (x0 - ax) / (bx - ax).max(1e-3);
                    pts.drain(..k - 1);
                    pts[0] = (x0, ay + (by - ay) * t);
                }
            }
            if let Some(p) = draw::polyline(&pts, false) {
                pix.stroke_path(&p, &draw::paint(col(c)), &draw::stroke(self.u(2.0), LineCap::Round), Transform::identity(), None);
            }
        };
        let i = idx(g);
        line(pix, &self.hist[i], self.accent[i]);
        if g == Glyph::Net {
            line(pix, &self.hist[8], 0xc084fc);
        }
        // Captions: model name top-left (CPU / GPU), full scale top-right (FPS: refresh rate).
        let (fnum, ty) = (self.font_num(), b.y + self.u(14.0)); // under the 100% line (gy(100) = +5)
        let name = match g {
            Glyph::Cpu => self.cpu_name.clone(),
            Glyph::Gpu => self.gpu_name.clone(),
            _ => String::new(),
        };
        self.label(pix, fnum, &name, x0 + self.u(4.0), ty, TRACK_LBL, Anchor::West);
        if g == Glyph::Fps && self.v.fps_hz > 0 {
            let t = format!("{} Hz", self.v.fps_hz);
            let w = self.text.measure(fnum, &t) as f32;
            self.label(pix, fnum, &t, x1 - w - self.u(4.0), ty, TRACK_LBL, Anchor::West);
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
        // Temperature rings ease like the snakes: arc, color and number move together.
        let mut gauges = false;
        for (k, target) in [self.v.cpu_temp, self.v.gpu_temp].into_iter().enumerate() {
            let was = self.gauge_moving(k);
            let t = &mut self.temps[k];
            *t = match (*t, target) {
                (Some(c), Some(g)) if (g - c).abs() > 0.05 => Some(c + (g - c) * ease),
                _ => target,
            };
            gauges |= self.gauge_moving(k);
            if was && !self.gauge_moving(k) {
                self.dirty = true; // settled: bake it into the static layer
                self.lines_dirty = true;
            }
        }
        if self.fade < 1.0 {
            self.fade = (self.fade + dt / FADE_S).min(1.0);
            self.lines_dirty |= self.fade >= 1.0; // land exactly on the new frame
        }
        // One 30 fps clock for every per-frame layer change (ribbon scroll, easing rings).
        let scrolling = self.phase < 1.0;
        if scrolling {
            self.phase = (self.phase + dt / SAMPLE_S).min(1.0);
        }
        if scrolling || gauges {
            self.line_acc += dt;
            if self.line_acc >= 1.0 / LINES_FPS || (scrolling && self.phase >= 1.0) {
                self.line_acc = 0.0;
                self.lines_dirty = true;
            }
        }
        let d = dt / ROW_ANIM_S;
        for (i, &g) in ROWS.iter().enumerate() {
            let t = if self.visible(g) { 1.0 } else { 0.0 };
            if self.open[i] != t {
                self.open[i] = if t > self.open[i] { (self.open[i] + d).min(t) } else { (self.open[i] - d).max(t) };
                self.dirty = true;
            }
        }
    }

    /// True while some visible snake moves (rotating segment or easing length).
    pub fn animating(&self) -> bool {
        self.lines_moving() || self.rows_moving() || self.icon_boxes.iter().any(|&(i, _)| {
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
        if self.lines_dirty {
            self.compose();
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
        let h = (self.snake_rows().map(|(_, b)| b.y + b.h + pad).fold(1.0, f32::max).ceil() as u32).min(self.stat.height());
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
