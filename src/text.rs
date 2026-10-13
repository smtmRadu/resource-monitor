//! Text via GDI (hinted Segoe UI Bold + ClearType, like the Tk version),
//! rendered as a white-on-black coverage mask and composited into the pixmap.
use std::collections::HashMap;

use tiny_skia::PixmapMut;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, SIZE};
use windows::Win32::Graphics::Gdi::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    West,
    Center,
}

struct Font {
    h: HFONT,
    asc: i32,
    desc: i32,
}

pub struct Text {
    dc: HDC,
    bmp: HBITMAP,
    bits: *mut u8,
    bw: i32,
    bh: i32,
    fonts: HashMap<i32, Font>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

impl Text {
    pub fn new() -> Self {
        let dc = unsafe { CreateCompatibleDC(None) };
        Self { dc, bmp: HBITMAP::default(), bits: std::ptr::null_mut(), bw: 0, bh: 0, fonts: HashMap::new() }
    }

    /// Bold Segoe UI with a pixel em height of `px`.
    fn font(&mut self, px: i32) -> &Font {
        let dc = self.dc;
        self.fonts.entry(px).or_insert_with(|| unsafe {
            let face: Vec<u16> = "Segoe UI".encode_utf16().chain(Some(0)).collect();
            let h = CreateFontW(
                -px, 0, 0, 0, FW_BOLD.0 as i32, 0, 0, 0,
                DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
                (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32, PCWSTR(face.as_ptr()),
            );
            let old = SelectObject(dc, h.into());
            let mut tm = TEXTMETRICW::default();
            let _ = GetTextMetricsW(dc, &mut tm);
            SelectObject(dc, old);
            Font { h, asc: tm.tmAscent, desc: tm.tmDescent }
        })
    }

    pub fn measure(&mut self, px: i32, s: &str) -> i32 {
        if s.is_empty() {
            return 0;
        }
        let h = self.font(px).h;
        let w = wide(s);
        let mut sz = SIZE::default();
        unsafe {
            let old = SelectObject(self.dc, h.into());
            let _ = GetTextExtentPoint32W(self.dc, &w, &mut sz);
            SelectObject(self.dc, old);
        }
        sz.cx
    }

    fn ensure(&mut self, w: i32, h: i32) {
        if w <= self.bw && h <= self.bh && !self.bits.is_null() {
            return;
        }
        let (w, h) = (w.max(self.bw).max(64), h.max(self.bh).max(16));
        unsafe {
            let bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let Ok(bmp) = CreateDIBSection(Some(self.dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0) else {
                return;
            };
            // Swap the new bitmap in first, then free the old one (it can't be
            // deleted while selected into the DC).
            SelectObject(self.dc, bmp.into());
            if !self.bmp.is_invalid() {
                let _ = DeleteObject(self.bmp.into());
            }
            self.bmp = bmp;
            self.bits = bits as *mut u8;
        }
        self.bw = w;
        self.bh = h;
    }

    /// Draw `s` with Tk anchor semantics: the line box is vertically centered on `yc`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, pix: &mut PixmapMut, px: i32, s: &str, x: f32, yc: f32, color: u32, anchor: Anchor) {
        if s.is_empty() {
            return;
        }
        let tw = self.measure(px, s);
        let (h, asc, desc) = {
            let f = self.font(px);
            (f.h, f.asc, f.desc)
        };
        let lh = asc + desc;
        let pad = 2;
        let (w, hh) = (tw + 2 * pad, lh);
        self.ensure(w, hh);
        if self.bits.is_null() {
            return;
        }
        let bw = self.bw as usize;
        unsafe {
            for y in 0..hh as usize {
                std::ptr::write_bytes(self.bits.add(y * bw * 4), 0, w as usize * 4);
            }
            let old = SelectObject(self.dc, h.into());
            SetBkMode(self.dc, TRANSPARENT);
            SetTextColor(self.dc, COLORREF(0x00ff_ffff));
            let ws = wide(s);
            let _ = TextOutW(self.dc, pad, 0, &ws);
            SelectObject(self.dc, old);
            let _ = GdiFlush();
        }
        let left = match anchor {
            Anchor::West => x.round() as i32,
            Anchor::Center => x.round() as i32 - tw / 2,
        } - pad;
        let top = yc.round() as i32 - lh / 2;
        let (fr, fg, fb) = ((color >> 16) & 255, (color >> 8) & 255, color & 255);
        let (pw, ph) = (pix.width() as i32, pix.height() as i32);
        let data = pix.data_mut();
        for yy in 0..hh {
            let dy = top + yy;
            if dy < 0 || dy >= ph {
                continue;
            }
            for xx in 0..w {
                let dx = left + xx;
                if dx < 0 || dx >= pw {
                    continue;
                }
                let si = (yy as usize * bw + xx as usize) * 4;
                let (cb, cg, cr) = unsafe { (*self.bits.add(si) as u32, *self.bits.add(si + 1) as u32, *self.bits.add(si + 2) as u32) };
                if cr | cg | cb == 0 {
                    continue;
                }
                let di = (dy as usize * pw as usize + dx as usize) * 4;
                let d = &mut data[di..di + 4];
                if d[3] == 255 {
                    // Opaque backdrop: per-channel ClearType blend.
                    d[0] = ((fr * cr + d[0] as u32 * (255 - cr)) / 255) as u8;
                    d[1] = ((fg * cg + d[1] as u32 * (255 - cg)) / 255) as u8;
                    d[2] = ((fb * cb + d[2] as u32 * (255 - cb)) / 255) as u8;
                } else {
                    // Translucent backdrop (game mode): grayscale src-over, premultiplied.
                    let a = (cr + cg + cb) / 3;
                    let inv = 255 - a;
                    d[0] = ((fr * a + d[0] as u32 * inv) / 255) as u8;
                    d[1] = ((fg * a + d[1] as u32 * inv) / 255) as u8;
                    d[2] = ((fb * a + d[2] as u32 * inv) / 255) as u8;
                    d[3] = (a + d[3] as u32 * inv / 255) as u8;
                }
            }
        }
    }
}
