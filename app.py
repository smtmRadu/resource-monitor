"""Draft UI: transparent always-on-top pill monitor, docked upper-left.

Controls:
  drag pills ......... move window      | double-click .... toggle core grid
  right-click ........ tray menu        | C key ........... toggle core grid
  H .................. hide to tray (runs in background)

Visual: dark pills (#14161c @ ~92% opacity) with colored status dots,
white Segoe UI text. No window chrome. Stays on top of games/browsers.
"""
from __future__ import annotations

import os as _os
_os.chdir(_os.path.dirname(_os.path.abspath(__file__)))  # stable cwd (task scheduler starts in System32)
with open("boot.log", "a") as _bf:
    import datetime as _dt
    _bf.write(f"{_dt.datetime.now():%H:%M:%S} boot pid={_os.getpid()}\n")

import threading
import tkinter as tk
from tkinter import font as tkfont

from PIL import Image, ImageDraw, ImageTk

import metrics

PILL, TXT = "#171a21", "#f2f4f8"
LIGHT_YELLOW = "#fef08a"  # wattage text (lightning pale yellow)
TRANSCOLOR = "#010203"  # chroma-key: anything painted this color is see-through
PILL_W, PILL_H = 210, 24
ICON_W = 79  # uniform icon-pill width for CPU/RAM/GPU/VRAM/iGPU (FPS excepted)
VAL_W = 167  # metrics-pill width (row total unchanged)
NORMAL_ALPHA = 0.93  # window opacity on desktop (never touched by game mode)
GAME_ALPHA = 64  # pill-background alpha (0-255) while a game runs; text/icons stay solid
GHOST_ALPHA = 128  # current pill-bg alpha (128 = 50% normal, 64 = 25% game)


def _snap(img):
    """Snap alpha to 1-bit so edges key out cleanly (no fringe crumbs).

    The window uses colorkey transparency: any half-blended edge pixel fails
    the key test and shows as a dark crumb on bright backgrounds. Snapping
    keeps smooth color AA inside shapes while edges cut cleanly.
    """
    r, g, b, a = img.split()
    a = a.point(lambda v: 255 if v > 110 else 0)
    return Image.merge("RGBA", (r, g, b, a))


def capsule_bg(w: int, h: int, alpha: int):
    """Stadium capsule image with real per-pixel alpha (for ghost mode)."""
    SS = 2
    img = Image.new("RGBA", (w * SS, h * SS), (0, 0, 0, 0))
    dr = ImageDraw.Draw(img)
    r, m = ((h - 4) // 2) * SS, 2 * SS
    W, H = w * SS, h * SS
    dr.ellipse([m, m, m + 2 * r, m + 2 * r], fill=(23, 26, 33, alpha))
    dr.rectangle([m + r, m, m + W - 2 * m - r, m + H - 2 * m], fill=(23, 26, 33, alpha))
    dr.ellipse([m + W - 2 * m - 2 * r, m, m + W - 2 * m, m + 2 * r], fill=(23, 26, 33, alpha))
    return _snap(img.resize((w, h), Image.LANCZOS))


SCALE = 1.0  # UI scale from tray menu (1x / 1.5x / 2x)
FONT_MAIN = ("Segoe UI", 10, "bold")
FONT_NUM = ("Segoe UI", 8, "bold")
IGPU_VARIANT = "C"  # user pick (locked)
FPS_VARIANT = "B"  # user pick: 270° sweep + redline (locked)
_CPU_V, _GPU_V = metrics.cpu_vendor(), metrics.gpu_vendor()
# vendor-aware icon colors: NVIDIA green / AMD orange / Intel blue
ACCENT = {"cpu": {"intel": "#60a5fa", "amd": "#f97316"}.get(_CPU_V, "#4ade80"),
          "ram": "#eab308",
          "gpu": {"nvidia": "#4ade80", "amd": "#f97316", "intel": "#60a5fa"}.get(_GPU_V, "#f472b6"),
          "vram": "#c084fc",
          "temp": {"intel": "#60a5fa", "amd": "#f97316"}.get(_CPU_V, "#fb923c"),
          "fps": "#facc15",
          "disk": "#94a3b8",
          "net": "#7dd3fc"}


def usage_color(p: float) -> str:
    """green -> yellow -> red lerp for core blocks."""
    p = max(0.0, min(100.0, p)) / 100.0
    if p < 0.5:
        r, g = int(74 + (250 - 74) * p * 2), 222
    else:
        r, g = 250, int(222 - (222 - 80) * (p - 0.5) * 2)
    return f"#{r:02x}{g:02x}50"


def _draw_icon(cv, key):
    """Detailed outline-style glyphs (transparent feel: strokes only, tiny hubs).
    Icon box: x 4..28, y 4..26, center (17, 15)."""
    c = ACCENT[key]
    O = {"outline": c, "width": 1, "tags": "icon"}
    F = {"fill": c, "outline": c, "tags": "icon"}
    L = lambda *a: cv.create_line(*a, fill=c, width=1, tags="icon")

    if key == "cpu":  # chip + pins on 4 sides
        cv.create_rectangle(10, 8, 24, 22, **O)
        cv.create_rectangle(14, 12, 20, 18, **O)
        for x in (12, 17, 22):
            L(x, 8, x, 5); L(x, 22, x, 25)
        for y in (10, 15, 20):
            L(10, y, 7, y); L(24, y, 27, y)
        cv.create_oval(16, 14, 18, 16, **F)

    elif key == "ram":  # memory stick: notch, teeth, chip
        cv.create_rectangle(7, 11, 27, 19, **O)
        L(12, 11, 12, 15)
        cv.create_rectangle(16, 13, 21, 17, **O)
        for x in (9, 14, 19, 24):
            L(x, 19, x, 22)

    elif key == "gpu":  # card + fan + bracket (mirrored: fan right, bracket left)
        cv.create_rectangle(7, 10, 28, 20, **O)
        cv.create_oval(17, 11, 25, 19, **O)
        for dx, dy in ((-2.2, -2.2), (2.2, -2.2), (-2.2, 2.2), (2.2, 2.2)):
            L(21, 15, 21 + dx, 15 + dy)
        cv.create_oval(20, 14, 22, 16, **F)
        cv.create_rectangle(9, 13, 15, 17, **O)
        L(6, 9, 6, 21)
        for x in (14, 11):
            L(x, 20, x, 23)

    elif key == "vram":  # stacked chips
        cv.create_rectangle(11, 7, 23, 17, **O)
        cv.create_rectangle(8, 11, 20, 23, **O)
        cv.create_oval(13, 15, 15, 17, **F)
        for x in (11, 14, 17):
            L(x, 23, x, 25)

    elif key == "temp":  # iGPU variants (picked live from tray menu)
        v = IGPU_VARIANT
        if v == "A":  # ring + die + pins (original)
            cv.create_oval(9, 7, 25, 23, **O)
            cv.create_rectangle(13, 11, 21, 19, **O)
            for x in (14, 20):
                L(x, 7, x, 4); L(x, 23, x, 26)
            for y in (12, 18):
                L(9, y, 6, y); L(25, y, 28, y)
            cv.create_oval(16, 14, 18, 16, **F)
        elif v == "B":  # ring + solid die (minimal)
            cv.create_oval(8, 6, 26, 24, **O)
            cv.create_rectangle(13, 11, 21, 19, **F)
        elif v == "C":  # mini chip (little sibling of CPU)
            cv.create_rectangle(11, 9, 23, 21, **O)
            cv.create_rectangle(14, 12, 20, 18, **O)
            for x in (14, 20):
                L(x, 9, x, 6); L(x, 21, x, 24)
            for y in (13, 18):
                L(11, y, 8, y); L(23, y, 26, y)
        elif v == "D":  # chiplet stack (two offset dice)
            cv.create_rectangle(12, 6, 24, 18, **O)
            cv.create_rectangle(8, 11, 20, 23, **O)
            cv.create_oval(13, 16, 15, 18, **F)
        else:  # E: onboard traces (die + circuit lines with pads)
            cv.create_rectangle(11, 9, 19, 17, **O)
            L(19, 12, 25, 12); cv.create_oval(24, 11, 26, 13, **F)
            L(15, 9, 15, 5); cv.create_oval(14, 4, 16, 6, **F)
            L(11, 14, 6, 14); cv.create_oval(5, 13, 7, 15, **F)

    elif key == "disk":  # drive: body + tray slot + LED
        cv.create_rectangle(6, 10, 24, 19, **O)
        L(6, 15, 24, 15)
        cv.create_oval(20, 11.5, 22, 13.5, **F)

    elif key == "net":  # globe: sphere + meridian + equator (compact)
        cv.create_oval(10, 8, 24, 22, **O)
        cv.create_oval(14, 8, 20, 22, **O)
        L(10, 15, 24, 15)

    elif key == "fps":  # speedometer family (picked live: tray / ctrl+shift+f)
        v = FPS_VARIANT
        RED = "#ef4444"
        if v == "A":  # classic top-half gauge
            cv.create_arc(9, 7, 25, 23, start=180, extent=180, style=tk.ARC, **O)
            L(9, 15, 12, 15); L(17, 7, 17, 10); L(25, 15, 22, 15)
            L(17, 15, 21.5, 9.5)
            cv.create_oval(16, 14, 18, 16, **F)
        elif v == "B":  # 270° wide sweep + redline zone
            cv.create_arc(9, 7, 25, 23, start=315, extent=270, style=tk.ARC, **O)
            cv.create_arc(9, 7, 25, 23, start=45, extent=45, style=tk.ARC,
                          outline=RED, width=1)
            L(23, 21, 21, 19); L(11, 21, 13, 19); L(17, 7, 17, 10)
            L(17, 15, 21, 8.5)
            cv.create_oval(16, 14, 18, 16, **F)
        elif v == "C":  # full-ring tachometer dial
            cv.create_oval(9, 7, 25, 23, **O)
            L(17, 7, 17, 10); L(17, 23, 17, 20)
            L(9, 15, 12, 15); L(25, 15, 22, 15)
            L(17, 15, 22, 10)
            cv.create_oval(16, 14, 18, 16, **F)
        elif v == "D":  # chunky sport (thick arc + block needle + red hub)
            cv.create_arc(9, 7, 25, 23, start=180, extent=180, style=tk.ARC,
                          outline=c, width=3)
            L(9, 15, 12, 15); L(17, 7, 17, 10); L(25, 15, 22, 15)
            cv.create_line(17, 15, 21.5, 9.5, fill=c, width=3)
            cv.create_oval(15.5, 13.5, 18.5, 17.5, fill=RED, outline=RED)
        else:  # E: tailed needle + 5 ticks
            cv.create_arc(9, 7, 25, 23, start=180, extent=180, style=tk.ARC, **O)
            L(9, 15, 12, 15); L(25, 15, 22, 15); L(17, 7, 17, 10)
            L(21.2, 10.8, 22.7, 9.3); L(12.8, 10.8, 11.3, 9.3)
            L(13.5, 17.5, 21.5, 9.5)
            cv.create_oval(16, 14, 18, 16, **F)


def _gauge_image(v: int, D: int = 24, frac: float | None = None):
    """Thermometer disc rendered at 10x and downscaled.

    Trick: everything bakes onto an OPAQUE pill-colored disc (uniform alpha =
    GHOST_ALPHA), so antialiased edges blend invisibly. Only the outer corners
    stay transparent (snapped, no fringe crumbs on bright backgrounds).
    """
    import math
    SS, W = 10, 20
    S = D * SS
    img = Image.new("RGBA", (S, S), (23, 26, 33, 255))
    dr = ImageDraw.Draw(img)
    cx = cy = S / 2
    r = S / 2 - W / 2 - 2
    bbox = [cx - r, cy - r, cx + r, cy + r]
    dr.ellipse(bbox, outline="#39404e", width=W)
    f = v / 99 if frac is None else frac
    if f > 0:
        col = usage_color(f * 100)
        a0, a1 = 270, 270 + f * 360
        dr.arc(bbox, start=a0, end=a1, fill=col, width=W)
        for a in (a0, a1):
            t = math.radians(a)
            ex, ey = cx + r * math.cos(t), cy + r * math.sin(t)
            dr.ellipse([ex - W / 2, ey - W / 2, ex + W / 2, ey + W / 2], fill=col)
    small = img.resize((D, D), Image.LANCZOS)
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).ellipse([2 * SS, 2 * SS, S - 2 * SS, S - 2 * SS], fill=255)
    m = mask.resize((D, D), Image.LANCZOS)
    a = m.point(lambda px: 0 if px < 110 else GHOST_ALPHA)
    small.putalpha(a)
    return small


def _stadium_points(w: float, h: float, pad: float = 2.0, n: int = 280,
                    ox: float = 0.0, oy: float = 0.0):
    """Sampled points around a stadium (pill) outline, starting top-left going clockwise."""
    import math
    r = h / 2 - pad
    cx0, cx1, cy = ox + h / 2, ox + w - h / 2, oy + h / 2
    straight = w - h
    total = 2 * straight + 2 * math.pi * r
    pts = []
    for i in range(n + 1):
        d = total * i / n
        if d < straight:
            pts.append((cx0 + d, oy + pad))
        elif d < straight + math.pi * r:
            a = (d - straight) / r - math.pi / 2
            pts.append((cx1 + r * math.cos(a), cy + r * math.sin(a)))
        elif d < 2 * straight + math.pi * r:
            pts.append((cx1 - (d - straight - math.pi * r), oy + h - pad))
        else:
            a = (d - 2 * straight - math.pi * r) / r + math.pi / 2
            pts.append((cx0 + r * math.cos(a), cy + r * math.sin(a)))
    return pts


def _blend(fg: str, bg: str, t: float) -> str:
    """Mix two #rrggbb colors; t=0 -> fg, t=1 -> bg."""
    f = tuple(int(fg[i:i + 2], 16) for i in (1, 3, 5))
    b = tuple(int(bg[i:i + 2], 16) for i in (1, 3, 5))
    return "#%02x%02x%02x" % tuple(int(f[i] + (b[i] - f[i]) * t) for i in range(3))


class Pill(tk.Canvas):
    """True pill shape drawn on a transparent canvas (no rectangular frame)."""

    def __init__(self, master, key: str, cores: int = 0, width: int = 0,
                 bare: bool = False, extras: bool = True, margin: int = 0):
        self._pw = width or PILL_W
        self._mg = margin  # transparent margin around capsule for the snake ring
        super().__init__(master, width=self._pw + 2 * margin, height=PILL_H + 2 * margin,
                         bg=TRANSCOLOR, bd=0, highlightthickness=0)
        self._key = key
        self._cores = cores
        self._bare = bare  # no icon; text starts at x=12 (metrics capsule)
        self._extras = extras  # False: no minis/gauge (icon+name capsule)
        self.centered = False  # FPS value pill centers its text
        m = self._mg
        self._per = _stadium_points(self._pw, PILL_H, pad=1.0, ox=m, oy=m)
        self._head = 0.0  # snake head position (0..1 around perimeter)
        self._slen = 0.0  # shown snake length, eases toward target
        self._starget = 0.0  # live percentage target (set by 1Hz data tick)
        self._minis: list[int] = []
        self._minis_on = True
        self._eth_down = self._eth_up = None
        self._last = "…"
        self._text = None
        self.refresh()

    def refresh(self):
        """Redraw capsule + icon (used when switching iGPU variant live)."""
        self.delete("all")
        w, h = self._pw, PILL_H
        # background capsule as alpha image (ghost mode fades bg only)
        self._bgphoto = ImageTk.PhotoImage(capsule_bg(w, h, GHOST_ALPHA))
        self.create_image(w // 2 + self._mg, h // 2 + self._mg, image=self._bgphoto)
        tx = int(12 * SCALE) if self._bare else int(34 * SCALE)
        if not self._bare:
            _draw_icon(self, self._key)
            self.scale("icon", 0, 0, SCALE, SCALE)
            self.move("icon", 0, PILL_H / 2 - 15 * SCALE)
        self._text = self.create_text(tx if not self.centered else w / 2,
                                      h // 2 + 1, text=self._last, fill=TXT,
                                      font=FONT_MAIN,
                                      anchor="w" if not self.centered else "center")
        self._tx = tx
        self._ty = h // 2 + 1
        self._sub = self.create_text(tx, h // 2 + 1, text="", fill=LIGHT_YELLOW,
                                     font=FONT_MAIN, anchor="w")
        if self._mg:
            self.move("all", self._mg, self._mg)  # center capsule, snake runs in margin
        self._draw_minis()
        self._draw_temp_gauge()
        self._draw_eth()

    def _draw_minis(self):
        """Tiny per-core squares inside the CPU pill, right after the text.
        Grid adapts: 4->2x2, 8->2x4, 16->2x8, 24->3x8, 32->4x8, 64->4x16."""
        self._minis = []
        n = self._cores
        if self._key != "cpu" or n < 1 or not self._extras:
            return
        if n <= 8:
            rows, cols = 2, (n + 1) // 2
        elif n <= 32:
            cols, rows = 8, (n + 7) // 8
        else:
            cols, rows = 16, (n + 15) // 16
        step = 4 * SCALE  # 3px squares, 1px gap (scaled)
        ms = 3 * SCALE
        ox = min(90 * SCALE, self._pw - PILL_H - 6 * SCALE - cols * step)
        oy = (PILL_H - (rows * step - 1) + 1) // 2  # optical vertical center
        for i in range(n):
            gr, gc = divmod(i, cols)
            x0, y0 = ox + gc * step, oy + gr * step
            self._minis.append(
                self.create_rectangle(x0, y0, x0 + ms, y0 + ms,
                                      fill="#22c55e", outline="", tags="mini"))
        if not self._minis_on:
            self.itemconfigure("mini", state="hidden")

    def minis_toggle(self):
        self._minis_on = not self._minis_on
        self.itemconfigure("mini",
                           state="normal" if self._minis_on else "hidden")

    def recenter_minis(self, text_end: float):
        """Center the micro grid between the text end and the temp gauge."""
        n = len(self._minis)
        if self._key != "cpu" or n < 1:
            return
        if n <= 8:
            rows, cols = 2, (n + 1) // 2
        elif n <= 32:
            cols, rows = 8, (n + 7) // 8
        else:
            cols, rows = 16, (n + 15) // 16
        step = 4 * SCALE
        gauge_x = self._pw - PILL_H
        strip_w = cols * step - 1
        ox = text_end + max(4 * SCALE, (gauge_x - text_end - strip_w) / 2)
        oy = (PILL_H - (rows * step - 1) + 1) // 2
        ms = 3 * SCALE
        for i, rid in enumerate(self._minis):
            gr, gc = divmod(i, cols)
            x0, y0 = ox + gc * step, oy + gr * step
            self.coords(rid, x0, y0, x0 + ms, y0 + ms)

    def _draw_eth(self):
        """Drawn ↓/↑ arrow icons + value slots in the ETH metrics pill."""
        self._eth_down = self._eth_up = None
        if self._key != "net" or not self._bare:
            return
        c = ACCENT["net"]
        UP = "#c084fc"  # upload arrow (VRAM purple)
        S = SCALE
        y = PILL_H / 2
        dx, ux = int(10 * S), int(64 * S)
        # down arrow: shaft + head
        self.create_rectangle(dx + 2.5 * S, y - 8 * S, dx + 4.5 * S, y - 1 * S,
                              fill=c, outline="")
        self.create_polygon(dx, y - 1 * S, dx + 7 * S, y - 1 * S, dx + 3.5 * S, y + 5 * S,
                            fill=c, outline="")
        # up arrow: shaft + head (purple)
        self.create_rectangle(ux + 2.5 * S, y + 1 * S, ux + 4.5 * S, y + 8 * S,
                              fill=UP, outline="")
        self.create_polygon(ux, y + 1 * S, ux + 7 * S, y + 1 * S, ux + 3.5 * S, y - 5 * S,
                            fill=UP, outline="")
        self._eth_down = self.create_text(dx + 10 * S, y + 1, text="", fill=TXT,
                                          font=FONT_MAIN, anchor="w")
        self._eth_up = self.create_text(ux + 10 * S, y + 1, text="", fill=TXT,
                                        font=FONT_MAIN, anchor="w")
        self.create_text(int(118 * S), y + 1, text="MB/s", fill="#9aa3b2",
                         font=FONT_MAIN, anchor="w")
        self.itemconfigure(self._text, text="")  # arrows carry the values
        self._last = ""

    def set_eth(self, down: str, up: str):
        if self._eth_down is not None:
            self.itemconfigure(self._eth_down, text=down)
            self.itemconfigure(self._eth_up, text=up)

    def snake(self, pct: float):
        """Set the live percentage target (drawn by the 20fps animate loop)."""
        self._starget = max(0.0, min(1.0, pct if pct is not None else 0.0))

    def snake_frame(self, speed: float = 0.009):
        """Advance + redraw the rotating stroke (called at ~20fps).

        Uniform color full-length segment; at ~100% a solid full ring.
        """
        target = self._starget
        self._slen += (target - self._slen) * 0.12
        self._head = (self._head + speed) % 1.0
        self.delete("snake")
        n = len(self._per) - 1
        col = ACCENT.get(self._key, "#fff")
        if self._slen > 0.985 and target > 0.985:
            flat = [c for p in (self._per + [self._per[0]]) for c in p]
            self.create_line(*flat, fill=col, width=2, smooth=True, tags="snake")
            return
        if self._slen < 0.01:
            return
        m = max(6, int(self._slen * n))
        coords = []
        for j in range(m + 1):
            idx = int(self._head * n - m + j) % n
            coords += [self._per[idx][0], self._per[idx][1]]
        self.create_line(*coords, fill=col, width=2, smooth=True, tags="snake")

    def _draw_temp_gauge(self):
        """Supersampled PIL ring (smooth) + canvas digits + fire."""
        self._timg = self._tnum = None
        self._tphoto = None
        self._tfire = []
        self._titems = []
        if self._key not in ("cpu", "gpu") or not self._extras:
            return
        d = PILL_H  # ring lives on the pill's right end disc (part of the pill)
        ccx, ccy = self._pw - PILL_H / 2, PILL_H / 2
        self._tphoto = ImageTk.PhotoImage(Image.new("RGBA", (d, d), (0, 0, 0, 0)))
        self._timg = self.create_image(ccx, ccy, image=self._tphoto)
        self._tnum = self.create_text(ccx, ccy - 1, text="",
                                      fill=TXT, font=FONT_NUM)
        cx, cy = float(ccx), float(ccy)
        # fire glyph for >99° ("burning"): outer flame + inner ember, hidden
        self._tfire = [
            self.create_polygon(cx - 4, cy + 5, cx - 4.2, cy + 1, cx - 2.5, cy - 1,
                                cx - 3.2, cy - 3.5, cx - 0.5, cy - 6, cx + 1.5, cy - 2.5,
                                cx + 4, cy - 0.5, cx + 3.5, cy + 5,
                                fill="#ef4444", outline="", tags="fire"),
            self.create_polygon(cx - 1.5, cy + 4, cx - 1.8, cy + 1, cx, cy - 1.5,
                                cx + 1.8, cy + 1.5, cx + 1.5, cy + 4,
                                fill="#facc15", outline="", tags="fire"),
        ]
        self.scale("fire", cx, cy, SCALE, SCALE)
        self._titems = [self._timg, self._tnum]
        for it in self._titems + self._tfire:
            self.itemconfigure(it, state="hidden")

    def update_temp(self, value):
        """Show/hide + refresh the thermometer ring. None hides it."""
        if self._timg is None:
            return
        if value is None:
            for it in self._titems + self._tfire:
                self.itemconfigure(it, state="hidden")
            return
        raw = float(value)
        v = max(0, min(99, int(round(raw))))
        # bar spans absolute 0-100; color follows the 40-100 heat scale
        frac = max(0.0, min(1.0, (raw - 40) / 59))
        self._tphoto = ImageTk.PhotoImage(_gauge_image(v, PILL_H, frac))  # keep ref!
        self.itemconfigure(self._timg, image=self._tphoto)
        for it in self._titems:
            self.itemconfigure(it, state="normal")
        self.itemconfigure(self._tnum, text=f"{v}")
        show_fire = raw > 99
        for it in self._tfire:
            self.itemconfigure(it, state="normal" if show_fire else "hidden")
        if show_fire:
            self.itemconfigure(self._tnum, state="hidden")

    def set(self, s: str):
        self._last = s
        if self._text is not None:
            self.itemconfigure(self._text, text=s)
        if self._sub is not None:
            self.itemconfigure(self._sub, text="")

    def set_split(self, main: str, sub: str):
        """Two-tone text: main in white, sub (wattage) in lightning yellow."""
        self.set(main)
        if self._sub is not None:
            fw = tkfont.Font(font=FONT_MAIN)
            self.coords(self._sub, self._tx + fw.measure(main + " "), self._ty)
            self.itemconfigure(self._sub, text=sub)

    def fit_text(self, s: str):
        """Resize an icon capsule to hug its label (game names change)."""
        fw = tkfont.Font(font=FONT_MAIN)
        w = int(34 * SCALE) + fw.measure(s) + int(12 * SCALE)
        if abs(w - self._pw) >= 2:
            self._pw = w
            self.config(width=w + 2 * self._mg)
            self._per = _stadium_points(self._pw, PILL_H, pad=1.0,
                                        ox=self._mg, oy=self._mg)
            self.refresh()


class Monitor:
    def __init__(self):
        self.root = tk.Tk()
        r = self.root
        r.title("Resource Monitor")
        r.overrideredirect(True)          # no chrome -> true transparency feel
        r.attributes("-topmost", True)    # always on top
        r.attributes("-alpha", 0.93)      # global transparency
        r.attributes("-transparentcolor", TRANSCOLOR)  # no container rectangle
        r.configure(bg=TRANSCOLOR)
        r.geometry("+10+10")              # 10px from left and top edges
        # click-through: window never grabs the mouse (tray toggles it back)
        r.bind("<Double-Button-1>", lambda e: self.toggle_cores())
        r.bind("<KeyPress-c>", lambda e: self.toggle_cores())
        r.bind("<KeyPress-C>", lambda e: self.toggle_cores())
        r.bind("<KeyPress-h>", lambda e: self.hide_to_tray())
        r.bind("<ButtonPress-3>", self._tray_menu_popup)
        self._click_through = True
        self.root.update_idletasks()
        self.set_click_through(True)  # applied again every tick (Tk may reset exstyle)

        self.wrap = tk.Frame(r, bg=TRANSCOLOR)
        self.wrap.pack(padx=0, pady=0, anchor="w")
        # one module per row, split in two pills: [icon + name] [metrics...]
        # CPU metrics pill also carries micro cores + end-disc thermometer;
        # GPU metrics pill the thermometer. FPS row appears only in-game.
        self._build_rows()

        self._tray = None
        self._start_tray()
        self._logged_temp = False
        self._stop = False
        threading.Thread(target=self._sampler, daemon=True).start()
        self.root.after(1200, self.tick)
        self.root.after(42, self._animate)  # snakes render at ~24fps, data stays 1Hz

    def _animate(self):
        """Fast render loop for snakes only (smooth rotation)."""
        try:
            for p in (self.i_cpu, self.i_ram, self.i_gpu,
                      self.i_vram, self.i_tmp, self.p_fps_icon):
                p.snake_frame()
        except Exception:
            pass
        finally:
            self.root.after(42, self._animate)

    def _build_rows(self):
        """(Re)build all rows; called once at startup and on scale change."""
        import collections
        wrap = self.wrap
        self._rows = []
        self._tracks = {}
        self._all_pills = []
        if not hasattr(self, "_hist"):
            self._hist = {k: collections.deque(maxlen=60)
                          for k in ("cpu", "ram", "gpu", "vram", "temp", "fps",
                                    "disk", "net", "net_up")}
        if not hasattr(self, "_track_on"):
            self._track_on = True
        self._fps_shown = True
        self._game_mode = False
        self._igpu_shown = True  # _set_igpu(False) below hides it properly
        self._igpu_zero = 999
        n_cores = len(metrics.cpu_per_core()) or 24
        self.i_cpu, self.p_cpu = self._duo(wrap, "cpu", "CPU", n_cores)
        self.cores_on = True
        self.i_ram, self.p_ram = self._duo(wrap, "ram", "RAM")
        self.i_gpu, self.p_gpu = self._duo(wrap, "gpu", "GPU")
        self.i_vram, self.p_vram = self._duo(wrap, "vram", "VRAM")
        self.i_tmp, self.p_tmp = self._duo(wrap, "temp", "iGPU")
        self.i_disk, self.p_disk = self._duo(wrap, "disk", "DISK")
        self.i_net, self.p_net = self._duo(wrap, "net", "ETH")
        self.p_fps_icon, self.p_fps = self._duo(wrap, "fps", "FPS", icon_text="W" * 12)
        # iGPU starts hidden; appears on activity, hides after 5s at 0%
        self._set_igpu(False)

    def set_scale(self, s: float):
        """Tray size switch: rebuild the whole UI at 1x / 1.5x / 2x."""
        global SCALE, PILL_H, ICON_W, VAL_W, FONT_MAIN, FONT_NUM
        SCALE = s
        PILL_H = int(24 * s)
        ICON_W = int(79 * s)
        VAL_W = int(167 * s)
        FONT_MAIN = ("Segoe UI", int(10 * s), "bold")
        FONT_NUM = ("Segoe UI", int(8 * s), "bold")
        self.wrap.destroy()
        self.wrap = tk.Frame(self.root, bg=TRANSCOLOR)
        self.wrap.pack(padx=0, pady=0, anchor="w")
        self._build_rows()
        self._fps_shown = True  # fresh rows start packed; tick corrects per game
        self._log(f"scale {s}x")

    def _duo(self, wrap, key: str, name: str, cores: int = 0, icon_text: str | None = None,
             vwidth: int = 0):
        """Row of [icon + name capsule] + [metrics capsule] + 60s track ribbon."""
        row = tk.Frame(wrap, bg=TRANSCOLOR)
        row.pack(anchor="w")
        fw = tkfont.Font(font=("Segoe UI", int(10 * SCALE), "bold"))
        if icon_text is not None:  # FPS: wide enough for a game name
            iw = int((34 + fw.measure(icon_text) + 12) * SCALE)
        else:
            iw = int(ICON_W)
        icon = Pill(row, key, width=iw, extras=False, margin=3)
        icon.pack(side="left", pady=int(3 * SCALE))
        icon.set(name)
        if key == "fps":  # fixed capsule fitting "999 FPS", centered text
            vw = int((fw.measure("999 FPS") + 20) * SCALE)
        else:
            vw = vwidth or int(VAL_W)
        val = Pill(row, key, cores=cores, width=vw, bare=True)
        if key == "fps":
            val.centered = True
            val.refresh()
        val.pack(side="left", padx=(int(6 * SCALE), 0), pady=int(3 * SCALE))
        # 60s history pill: fixed standard width for ALL graphs (FPS matches
        # the others), double ribbon height, small-pill corner radius
        track_w = int(ICON_W) + 2 * 3 + int(6 * SCALE) + int(VAL_W)
        track = tk.Canvas(wrap, width=track_w, height=int(60 * SCALE),
                          bg=TRANSCOLOR, bd=0, highlightthickness=0)
        self._rows.append((row, track, key))
        self._tracks[key] = track
        self._all_pills += [icon, val]
        if (self._track_on and (key != "fps" or self._fps_shown)
                and (key != "temp" or self._igpu_shown)):
            track.pack(anchor="w", pady=(1, int(4 * SCALE)))
        return icon, val

    def _set_igpu(self, show: bool):
        """Show/hide the iGPU row (+ its track when Track is on)."""
        if show and not self._igpu_shown:
            self._igpu_shown = True
            self._layout()
        elif not show and self._igpu_shown:
            self._igpu_shown = False
            self._layout()

    def _draw_track(self, key: str, cv, s2=None, c2="#bae6fd"):
        """60s history pill, fully rendered in PIL at 3x and downscaled (smooth)."""
        vals = list(self._hist.get(key, []))
        w, h = int(cv.cget("width")), int(cv.cget("height"))
        SS = 3
        S = (w * SS, h * SS)
        img = Image.new("RGBA", (S[0], S[1]), (0, 0, 0, 0))
        dr = ImageDraw.Draw(img)
        R = int(PILL_H / 2 * SS)
        m = 2 * SS
        dr.rounded_rectangle([m, m, S[0] - m - 1, S[1] - m - 1], radius=R,
                             fill=(23, 26, 33, GHOST_ALPHA))

        def _gy(p):
            return (h - 5 - p / 100 * (h - 10)) * SS

        x0, x1 = 6 * SS, S[0] - 6 * SS
        dr.line([x0, _gy(50), x1, _gy(50)], fill="#232833", width=SS)
        for p in (25, 75):
            y = _gy(p)
            x = x0
            while x < x1:
                dr.line([x, y, min(x + 8 * SS, x1), y], fill="#232833", width=SS)
                x += 16 * SS
        n = len(vals)

        def _map(vv):
            m = len(vv)
            pts = []
            for i, v in enumerate(vv):
                px = 6 * SS + i * (S[0] - 12 * SS) / max(1, m - 1)
                py = (h - 5 - max(0.0, min(100.0, v)) / 100 * (h - 10)) * SS
                pts += [px, py]
            return pts

        if n >= 2:
            dr.line(_map(vals), fill=ACCENT.get(key, "#fff"), width=2 * SS, joint="curve")
        if s2 and len(s2) >= 2:
            dr.line(_map(s2), fill=c2, width=2 * SS, joint="curve")
        photo = ImageTk.PhotoImage(_snap(img.resize((w, h), Image.LANCZOS)))
        cv._photo = photo  # keep ref!
        if getattr(cv, "_img_id", None) is None:
            cv._img_id = cv.create_image(w // 2, h // 2, image=photo)
        else:
            cv.itemconfigure(cv._img_id, image=photo)

    def toggle_track(self):
        self._track_on = not self._track_on
        self._layout()

    def _layout(self):
        """Single place packing rows + tracks in canonical order."""
        for row, track, key in self._rows:
            if key == "fps" and not self._fps_shown:
                row.forget()
                track.forget()
                continue
            if key == "temp" and not self._igpu_shown:
                row.forget()
                track.forget()
                continue
            row.pack(anchor="w")
            show = self._track_on
            if show:
                track.pack(anchor="w", pady=(1, int(4 * SCALE)))
            else:
                track.forget()

    # -- window helpers (fixed position, click-through, never draggable) --
    def _toplevel_hwnd(self):
        """Real top-level HWND (winfo_id is a child window; style bits go on top)."""
        import ctypes
        hwnd = ctypes.windll.user32.FindWindowW(None, "Resource Monitor")
        return hwnd or self.root.winfo_id()

    def set_click_through(self, on: bool):
        import ctypes
        try:
            hwnd = self._toplevel_hwnd()
            GWL_EXSTYLE, WS_EX_TRANSPARENT = -20, 0x20
            get = ctypes.windll.user32.GetWindowLongW
            set_ = ctypes.windll.user32.SetWindowLongW
            style = get(hwnd, GWL_EXSTYLE)
            set_(hwnd, GWL_EXSTYLE,
                 (style | WS_EX_TRANSPARENT) if on else (style & ~WS_EX_TRANSPARENT))
            self._click_through = on
        except Exception as e:
            try:
                with open("clickthrough.log", "a") as f:
                    f.write(f"click-through failed: {e!r}\n")
            except Exception:
                pass

    def _enforce_click_through(self):
        """Re-assert WS_EX_TRANSPARENT; Tk can wipe exstyle bits behind our back."""
        if not self._click_through:
            return
        try:
            import ctypes
            hwnd = self._toplevel_hwnd()
            if not (ctypes.windll.user32.GetWindowLongW(hwnd, -20) & 0x20):
                self.set_click_through(True)
        except Exception:
            pass

    def toggle_cores(self):
        self.cores_on = not self.cores_on
        self.p_cpu.minis_toggle()

    def set_igpu_variant(self, v: str):
        global IGPU_VARIANT
        IGPU_VARIANT = v
        self.p_tmp.refresh()

    def set_fps_variant(self, v: str):
        global FPS_VARIANT
        FPS_VARIANT = v
        self.p_fps.refresh()

    def hide_to_tray(self):
        self.root.withdraw()  # keeps process alive in background

    # -- tray (background mode, like Discord/NV panel) --
    def _log(self, msg: str):
        try:
            with open("app.log", "a") as f:
                import datetime
                f.write(f"{datetime.datetime.now():%H:%M:%S} {msg}\n")
        except Exception:
            pass

    def _start_tray(self):
        try:
            import pystray
            from PIL import Image, ImageDraw
            # logo: CPU chip fused with FPS sweep (line-icon style, like pills)
            S = 64
            G, Y, R = "#4ade80", "#facc15", "#ef4444"
            img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
            d = ImageDraw.Draw(img)
            d.rounded_rectangle([0, 0, S - 1, S - 1], radius=12,
                                fill="#171a21", outline="#39404e", width=2)
            d.rectangle([10, 10, 54, 54], outline=G, width=4)
            for x in (20, 32, 44):
                d.line([x, 10, x, 4], fill=G, width=4)
                d.line([x, 54, x, 60], fill=G, width=4)
            for y in (20, 32, 44):
                d.line([10, y, 4, y], fill=G, width=4)
                d.line([54, y, 60, y], fill=G, width=4)
            d.arc([24, 24, 40, 40], start=135, end=405, fill=Y, width=4)
            d.arc([24, 24, 40, 40], start=300, end=360, fill=R, width=4)
            d.line([32, 32, 37, 27], fill=Y, width=4)
            d.ellipse([29, 29, 35, 35], fill=G)
            menu = pystray.Menu(
                pystray.MenuItem("Size", pystray.Menu(
                    pystray.MenuItem("1x", lambda: self.root.after(0, self.set_scale, 1.0),
                                     checked=lambda item: SCALE == 1.0),
                    pystray.MenuItem("1.5x", lambda: self.root.after(0, self.set_scale, 1.5),
                                     checked=lambda item: SCALE == 1.5),
                    pystray.MenuItem("2x", lambda: self.root.after(0, self.set_scale, 2.0),
                                     checked=lambda item: SCALE == 2.0),
                )),
                pystray.MenuItem("Track", lambda: self.root.after(0, self.toggle_track),
                                 checked=lambda item: self._track_on),
                pystray.MenuItem("Quit", lambda: self.root.after(0, self.root.destroy)),
            )
            self._tray = pystray.Icon("resmon", img, "Resource Monitor", menu)
            threading.Thread(target=self._tray.run, daemon=True).start()
            self._log("tray started")
        except Exception as e:
            self._log(f"tray FAILED: {e!r}")

    def _tray_menu_popup(self, e):
        self.hide_to_tray()  # right-click = quick hide; full menu lives in tray

    # -- live update: ONE persistent sampler thread (psutil baselines are
    # per-thread; spawning a thread per tick returned 0.0 forever) --
    def tick(self):
        try:
            self._enforce_click_through()
        finally:
            self.root.after(1000, self.tick)

    def _sampler(self):
        import time
        n = 0
        while not getattr(self, "_stop", False):
            t0 = time.time()
            n += 1
            try:
                if n <= 3:
                    self._log(f"tick {n} snapshot START")
                s = metrics.snapshot()
                if n <= 3:
                    self._log(f"tick {n} snapshot END game={s['game']} fps={s['fps']}")
                self._log(f"tick {n} cpu={s['cpu']} cores0={s['cores'][0] if s['cores'] else 'NA'}")
                self._pending = s
            except Exception as e:
                self._pending = e
            finally:
                self.root.after(0, self._apply)
            dt = time.time() - t0
            time.sleep(max(0.2, 1.0 - dt))

    def _apply(self):
        s = getattr(self, "_pending", None)
        self._pending = None
        if s is None:
            return
        if isinstance(s, Exception):
            self._log(f"tick FAILED: {s!r}")
            self.p_cpu.set(f"ERR {s}"[:24])
            return
        try:
            f = lambda v, fmt=".0f": ("--" if v is None else format(v, fmt))

            def pct(v):
                if v is None:
                    return "--"
                return f"{v:.1f}%" if v < 10 else f"{v:.0f}%"
            cpu_w = s["cpu_watts"] if (s["cpu_watts"] or 0) > 0 else None  # 0.0 = unread
            _cm, _cs = f'{pct(s["cpu"])}', (f'{cpu_w:.0f}W' if cpu_w is not None else "")
            self.p_cpu.set_split(_cm, _cs)
            _fw = tkfont.Font(font=FONT_MAIN)
            self.p_cpu.recenter_minis(int(12 * SCALE) + _fw.measure(_cm + " " + _cs))
            self.p_cpu.update_temp(s["cpu_temp"])
            ram = s["ram"]
            self.p_ram.set(f'{pct(ram["percent"])} {ram["used_gb"]:.1f}/{ram["total_gb"]:.0f}G')
            gmain = pct(s["gpu_util"])
            gsub = ""
            if s["gpu_watts"] is not None:
                gsub = f'{s["gpu_watts"]:.0f}'
                gsub += f'/{s["gpu_limit_w"]:.0f}' if s["gpu_limit_w"] else ""
                gsub += "W"
            self.p_gpu.set_split(gmain, gsub)
            self.p_gpu.update_temp(s["gpu_temp"])
            vpct = (s["vram_used_mb"] / s["vram_total_mb"] * 100
                    if (s["vram_used_mb"] and s["vram_total_mb"]) else None)
            if vpct is not None:
                self.p_vram.set(f'{pct(vpct)} {s["vram_used_mb"]/1024:.1f}/{s["vram_total_mb"]/1024:.0f}G')
            else:
                self.p_vram.set("--")
            # iGPU auto-hide: visible on activity, gone after 5s at 0%
            ig = s["igpu"]
            if ig is not None and ig > 0:
                self._igpu_zero = 0
                self._set_igpu(True)
                self.p_tmp.set(f'{pct(ig)}')
            else:
                self.p_tmp.set("0.0%")
                self._igpu_zero = getattr(self, "_igpu_zero", 99) + 1
                if self._igpu_zero >= 5:
                    self._set_igpu(False)
            # FPS row only exists while a game runs (hidden otherwise)
            if s["game"]:
                disp = s["game"]
                if disp.lower().endswith(".exe"):
                    disp = disp[:-4]
                self.p_fps_icon.fit_text(disp[:12])
                self.p_fps_icon.set(disp[:12])
                # value capsule fills the remainder so the row hits standard width
                std = int(ICON_W) + int(6 * SCALE) + int(VAL_W)
                vw = std - int(6 * SCALE) - self.p_fps_icon._pw
                if abs(vw - self.p_fps._pw) >= 2 and vw >= int(60 * SCALE):
                    self.p_fps._pw = vw
                    self.p_fps.config(width=vw)
                    self.p_fps.refresh()
                self.p_fps.set(f'{s["fps"]} FPS' if s["fps"] else "--")
                self.p_fps_icon.snake((s["fps"] / metrics.display_hz()) if s["fps"] else 0.0)
                if not self._fps_shown:
                    self._fps_shown = True
                    self._layout()
            elif self._fps_shown:
                self._fps_shown = False
                self._layout()
            else:
                self.p_fps_icon.snake(0.0)
            for rid, v in zip(self.p_cpu._minis, s["cores"]):
                self.p_cpu.itemconfig(rid, fill=usage_color(v))
            # rotating snakes on the icon pills; length = live percentage
            self.i_cpu.snake(s["cpu"] / 100)
            self.i_ram.snake(ram["percent"] / 100)
            self.i_gpu.snake((s["gpu_util"] or 0) / 100)
            self.i_vram.snake((vpct / 100) if vpct is not None else 0.0)
            self.i_tmp.snake((s["igpu"] or 0) / 100)
            # game-mode ghost: only pill BACKGROUNDS fade (text/icons/rings/tracks stay solid)
            want_game = s["game"] is not None
            if want_game != self._game_mode:
                self._game_mode = want_game
                global GHOST_ALPHA
                GHOST_ALPHA = GAME_ALPHA if want_game else 128
                for p in self._all_pills:
                    p.refresh()
            # Track ribbons: record 60s rolling history, redraw if visible
            hz = metrics.display_hz()
            self._hist["cpu"].append(s["cpu"])
            self._hist["ram"].append(ram["percent"])
            self._hist["gpu"].append(s["gpu_util"] or 0)
            if vpct is not None:
                self._hist["vram"].append(vpct)
            if s["igpu"] is not None:
                self._hist["temp"].append(s["igpu"])
            if s["fps"]:
                self._hist["fps"].append(min(100.0, s["fps"] / hz * 100))
            # DISK + ETH pills (transfer rate / down+up speeds)
            self.p_disk.set(f'{s["disk_mbs"]:.1f}MB/s')
            self.p_net.set_eth(f'{s["net_down"]:.1f}', f'{s["net_up"]:.1f}')
            self._hist["disk"].append(min(100.0, s["disk_mbs"] * 0.2))  # ref 500MB/s
            self._hist["net"].append(min(100.0, s["net_down"]))  # ref 100MB/s
            self._hist["net_up"].append(min(100.0, s["net_up"]))
            if self._track_on:
                for k, cv in self._tracks.items():
                    if k == "fps" and not self._fps_shown:
                        continue
                    if k == "net":
                        self._draw_track(k, cv, list(self._hist["net_up"]), "#c084fc")
                    else:
                        self._draw_track(k, cv)
            if not self._logged_temp and (s["cpu_temp"] is not None or (s["cpu_watts"] or 0) > 0):
                self._logged_temp = True
                self._log(f"CPU temp/W LIVE: {s['cpu_temp']}C {s['cpu_watts']}W")
            if not getattr(self, "_logged_lenovo", False) and s.get("lenovo"):
                self._logged_lenovo = True
                self._log(f"LENOVO: {s['lenovo']}")
        except Exception as e:
            import traceback
            self._log(f"tick FAILED: {e!r} | {traceback.format_exc(limit=3).replace(chr(10), ' ')}")
            self.p_cpu.set(f"ERR {e}"[:24])

    def run(self):
        self.root.mainloop()
        self._stop = True
        if self._tray:
            try:
                self._tray.stop()
            except Exception:
                pass


if __name__ == "__main__":
    Monitor().run()
