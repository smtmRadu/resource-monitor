//! tiny-skia helpers and the outline-style pill icons.
use tiny_skia::*;

pub const PILL: u32 = 0x171a21;
pub const TXT: u32 = 0xf2f4f8;
pub const WATT: u32 = 0xfef08a;
pub const MUTED: u32 = 0x9aa3b2;
pub const RED: u32 = 0xef4444;

pub fn col(hex: u32) -> Color {
    col_a(hex, 1.0)
}

pub fn col_a(hex: u32, a: f32) -> Color {
    Color::from_rgba8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8, (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

pub fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

pub fn stroke(width: f32, cap: LineCap) -> Stroke {
    Stroke { width, line_cap: cap, line_join: LineJoin::Round, ..Default::default() }
}

/// green -> yellow -> red for 0..100 %.
pub fn usage_color(p: f32) -> u32 {
    let p = p.clamp(0.0, 100.0) / 100.0;
    let (r, g) = if p < 0.5 {
        ((74.0 + (250.0 - 74.0) * p * 2.0) as u32, 222)
    } else {
        (250, (222.0 - (222.0 - 80.0) * (p - 0.5) * 2.0) as u32)
    };
    (r << 16) | (g << 8) | 0x50
}

pub fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let k = 0.552_284_8 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

pub fn fill_rrect(pix: &mut PixmapMut, x: f32, y: f32, w: f32, h: f32, r: f32, c: Color) {
    if let Some(p) = rrect(x, y, w, h, r) {
        pix.fill_path(&p, &paint(c), FillRule::Winding, Transform::identity(), None);
    }
}

pub fn fill_rect(pix: &mut PixmapMut, x: f32, y: f32, w: f32, h: f32, c: Color) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        let mut p = paint(c);
        p.anti_alias = false;
        pix.fill_rect(r, &p, Transform::identity(), None);
    }
}

/// Elliptical arc as a polyline; angles in degrees, math convention
/// (counter-clockwise from 3 o'clock, screen y pointing down).
pub fn arc_path(cx: f32, cy: f32, rx: f32, ry: f32, start: f32, extent: f32) -> Option<Path> {
    let steps = ((extent.abs() / 4.0).ceil() as usize).max(2);
    let mut pb = PathBuilder::new();
    for i in 0..=steps {
        let a = (start + extent * i as f32 / steps as f32).to_radians();
        let (x, y) = (cx + rx * a.cos(), cy - ry * a.sin());
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    pb.finish()
}

pub fn polyline(pts: &[(f32, f32)], closed: bool) -> Option<Path> {
    let mut pb = PathBuilder::new();
    for (i, &(x, y)) in pts.iter().enumerate() {
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    if closed {
        pb.close();
    }
    pb.finish()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Glyph {
    Cpu,
    Ram,
    Gpu,
    Vram,
    Igpu,
    Disk,
    Net,
    Fps,
    /// NVDA stock row (its own window, docked above the taskbar).
    Nvda,
}

/// NVIDIA's eye mark: the real logo outline (Wikimedia Commons "NVIDIA logo.svg", path21),
/// in its own units (x 56.8..305.7, y 31.5..196.2).
const NV_EYE: &str = "M82.2115,102.414C82.2115,102.414 104.7155,69.211 149.6485,65.777L149.6485,53.73C99.8795,57.727 56.7818,99.879 56.7818,99.879C56.7818,99.879 81.1915,170.445 149.6485,176.906L149.6485,164.102C99.4105,157.781 82.2115,102.414 82.2115,102.414zM149.6485,138.637L149.6485,150.363C111.6805,143.594 101.1415,104.125 101.1415,104.125C101.1415,104.125 119.3715,83.93 149.6485,80.656L149.6485,93.523C149.6255,93.523 149.6095,93.516 149.5905,93.516C133.6995,91.609 121.2855,106.453 121.2855,106.453C121.2855,106.453 128.2425,131.445 149.6485,138.637M149.6485,31.512L149.6485,53.73C151.1095,53.617 152.5705,53.523 154.0395,53.473C210.6215,51.566 247.4885,99.879 247.4885,99.879C247.4885,99.879 205.1455,151.367 161.0315,151.367C156.9885,151.367 153.2035,150.992 149.6485,150.363L149.6485,164.102C152.6885,164.488 155.8405,164.715 159.1295,164.715C200.1805,164.715 229.8675,143.75 258.6135,118.937C263.3795,122.754 282.8915,132.039 286.9025,136.105C259.5705,158.988 195.8715,177.434 159.7585,177.434C156.2775,177.434 152.9345,177.223 149.6485,176.906L149.6485,196.211L305.6805,196.211L305.6805,31.512L149.6485,31.512zM149.6485,80.656L149.6485,65.777C151.0945,65.676 152.5515,65.598 154.0395,65.551C194.7275,64.273 221.4225,100.516 221.4225,100.516C221.4225,100.516 192.5905,140.559 161.6765,140.559C157.2275,140.559 153.2385,139.844 149.6485,138.637L149.6485,93.523C165.4885,95.437 168.6765,102.434 178.1995,118.309L199.3795,100.449C199.3795,100.449 183.9185,80.172 157.8555,80.172C155.0205,80.172 152.3095,80.371 149.6485,80.656";

/// Minimal SVG path reader: absolute M / L / C / Z only (all NV_EYE uses).
fn svg_path(d: &str) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let mut rest = d.trim();
    while let Some(cmd) = rest.chars().next() {
        let end = rest[1..].find(|c: char| c.is_ascii_alphabetic()).map_or(rest.len(), |i| i + 1);
        let nums: Vec<f32> = rest[1..end].split(|c: char| c == ',' || c.is_whitespace()).filter_map(|t| t.parse().ok()).collect();
        match cmd {
            'M' => {
                for (i, c) in nums.chunks_exact(2).enumerate() {
                    if i == 0 {
                        pb.move_to(c[0], c[1]);
                    } else {
                        pb.line_to(c[0], c[1]);
                    }
                }
            }
            'L' => nums.chunks_exact(2).for_each(|c| pb.line_to(c[0], c[1])),
            'C' => nums.chunks_exact(6).for_each(|c| pb.cubic_to(c[0], c[1], c[2], c[3], c[4], c[5])),
            'Z' | 'z' => pb.close(),
            _ => return None,
        }
        rest = &rest[end..];
    }
    pb.finish()
}

fn nv_eye() -> Option<&'static Path> {
    static EYE: std::sync::OnceLock<Option<Path>> = std::sync::OnceLock::new();
    EYE.get_or_init(|| svg_path(NV_EYE)).as_ref()
}

/// Icon pen in the original 34x30 icon grid (box 4..28, center 17,15).
/// Coordinates are pixel-index based like the Tk canvas: +0.5 hits pixel centers.
struct Pen<'a, 'b> {
    pix: &'a mut PixmapMut<'b>,
    ts: Transform,
    c: Color,
}

impl Pen<'_, '_> {
    fn stroke_path(&mut self, p: Option<Path>, c: Color, w: f32) {
        if let Some(p) = p {
            self.pix.stroke_path(&p, &paint(c), &stroke(w, LineCap::Butt), self.ts, None);
        }
    }
    fn r(&mut self, x1: f32, y1: f32, x2: f32, y2: f32) {
        let p = Rect::from_ltrb(x1 + 0.5, y1 + 0.5, x2 + 0.5, y2 + 0.5).map(PathBuilder::from_rect);
        self.stroke_path(p, self.c, 1.0);
    }
    /// Line from p1 up to (not including) p2, like GDI.
    fn l(&mut self, x1: f32, y1: f32, x2: f32, y2: f32) {
        let (dx, dy) = (x2 - x1, y2 - y1);
        let len = (dx * dx + dy * dy).sqrt().max(1e-3);
        let (ux, uy) = (dx / len * 0.5, dy / len * 0.5);
        let p = polyline(&[(x1 + 0.5 - ux, y1 + 0.5 - uy), (x2 + 0.5 - ux, y2 + 0.5 - uy)], false);
        self.stroke_path(p, self.c, 1.0);
    }
    fn o(&mut self, x1: f32, y1: f32, x2: f32, y2: f32) {
        let p = Rect::from_ltrb(x1 + 0.5, y1 + 0.5, x2 + 0.5, y2 + 0.5).and_then(PathBuilder::from_oval);
        self.stroke_path(p, self.c, 1.0);
    }
    fn f(&mut self, x1: f32, y1: f32, x2: f32, y2: f32) {
        if let Some(p) = Rect::from_ltrb(x1, y1, x2 + 1.0, y2 + 1.0).and_then(PathBuilder::from_oval) {
            self.pix.fill_path(&p, &paint(self.c), FillRule::Winding, self.ts, None);
        }
    }
    /// Quadratic curve; points already on pixel centers (no +0.5).
    fn quad(&mut self, a: (f32, f32), c: (f32, f32), b: (f32, f32)) {
        let mut pb = PathBuilder::new();
        pb.move_to(a.0, a.1);
        pb.quad_to(c.0, c.1, b.0, b.1);
        self.stroke_path(pb.finish(), self.c, 1.0);
    }
    /// Filled disc; center already on pixel centers.
    fn dot(&mut self, cx: f32, cy: f32, r: f32) {
        if let Some(p) = PathBuilder::from_circle(cx, cy, r) {
            self.pix.fill_path(&p, &paint(self.c), FillRule::Winding, self.ts, None);
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn arc(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, start: f32, extent: f32, c: Color) {
        let (cx, cy) = ((x1 + x2) / 2.0 + 0.5, (y1 + y2) / 2.0 + 0.5);
        let p = arc_path(cx, cy, (x2 - x1) / 2.0, (y2 - y1) / 2.0, start, extent);
        self.stroke_path(p, c, 1.0);
    }
}

/// Draw `g` with its grid origin at (tx, ty), scaled by `s`.
pub fn icon(pix: &mut PixmapMut, g: Glyph, tx: f32, ty: f32, s: f32, c: Color) {
    let mut p = Pen { pix, ts: Transform::from_row(s, 0.0, 0.0, s, tx, ty), c };
    match g {
        Glyph::Cpu => {
            p.r(10., 8., 24., 22.);
            p.r(14., 12., 20., 18.);
            for x in [12., 17., 22.] {
                p.l(x, 8., x, 5.);
                p.l(x, 22., x, 25.);
            }
            for y in [10., 15., 20.] {
                p.l(10., y, 7., y);
                p.l(24., y, 27., y);
            }
            p.f(16., 14., 18., 16.);
        }
        Glyph::Ram => {
            p.r(7., 11., 27., 19.);
            p.l(12., 11., 12., 15.);
            p.r(16., 13., 21., 17.);
            for x in [9., 14., 19., 24.] {
                p.l(x, 19., x, 22.);
            }
        }
        Glyph::Gpu => {
            p.r(7., 10., 28., 20.);
            p.o(17., 11., 25., 19.);
            // Fan: 4 swept blades (a spiral) around the hub.
            let (cx, cy, r) = (21.5f32, 15.5f32, 4.0f32);
            let at = |a: f32, d: f32| (cx + d * a.cos(), cy + d * a.sin());
            for k in 0..4 {
                let a = (k as f32 * 90.0 - 90.0).to_radians();
                p.quad(at(a, 1.2), at(a + 0.3, r * 0.85), at(a + 1.05, r - 0.4));
            }
            p.dot(cx, cy, 1.0);
            p.r(9., 13., 15., 17.);
            p.l(6., 9., 6., 21.);
            for x in [14., 11.] {
                p.l(x, 20., x, 23.);
            }
        }
        Glyph::Vram => {
            p.r(11., 7., 23., 17.);
            p.r(8., 11., 20., 23.);
            p.f(13., 15., 15., 17.);
            for x in [11., 14., 17.] {
                p.l(x, 23., x, 25.);
            }
        }
        Glyph::Igpu => {
            p.r(11., 9., 23., 21.);
            p.r(14., 12., 20., 18.);
            for x in [14., 20.] {
                p.l(x, 9., x, 6.);
                p.l(x, 21., x, 24.);
            }
            for y in [13., 18.] {
                p.l(11., y, 8., y);
                p.l(23., y, 26., y);
            }
        }
        Glyph::Disk => {
            p.r(6., 10., 24., 19.);
            p.l(6., 15., 24., 15.);
            p.f(20., 11.5, 22., 13.5);
        }
        Glyph::Net => {
            p.o(10., 8., 24., 22.);
            p.o(14., 8., 20., 22.);
            p.l(10., 15., 24., 15.);
        }
        Glyph::Fps => {
            let c = p.c;
            p.arc(9., 7., 25., 23., 315., 270., c);
            p.arc(9., 7., 25., 23., 45., 45., col(RED));
            p.l(23., 21., 21., 19.);
            p.l(11., 21., 13., 19.);
            p.l(17., 7., 17., 10.);
            p.l(17., 15., 21., 8.5);
            p.f(16., 14., 18., 16.);
        }
        Glyph::Nvda => {
            // The eye mark, 20x13 grid px centered on the icon box, as a thin outline.
            if let Some(eye) = nv_eye() {
                let fit = Transform::from_translate(17.5, 15.5).pre_scale(0.08, 0.08).pre_translate(-181.25, -113.86);
                if let Some(path) = eye.clone().transform(p.ts.pre_concat(fit)) {
                    p.pix.stroke_path(&path, &paint(p.c), &stroke(0.7 * s, LineCap::Butt), Transform::identity(), None);
                }
            }
        }
    }
}
