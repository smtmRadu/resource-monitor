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
            for (dx, dy) in [(-2.2, -2.2), (2.2, -2.2), (-2.2, 2.2), (2.2, 2.2)] {
                p.l(21., 15., 21. + dx, 15. + dy);
            }
            p.f(20., 14., 22., 16.);
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
    }
}
