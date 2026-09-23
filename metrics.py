"""Lightweight Windows metrics collector (stdlib + psutil + nvidia-smi + WMI/PDH).

Proven on: Intel Core Ultra 9 275HX (24c/24t) + RTX 5070 Ti Laptop + Intel iGPU.
- CPU % / per-core % / freq : psutil              -> WORKING
- RAM % / used GB           : psutil              -> WORKING
- dGPU util / VRAM / temp / : nvidia-smi          -> WORKING
  power
- iGPU util                 : WMI GPUEngine       -> WORKING (aggregated, optional)
- CPU temp / package watts  : LibreHardwareMonitor-> PENDING (needs LHM running;
  : graceful fallback to None, shows '--' in UI)   shows '--' until phase 2
- FPS                       : placeholder         -> PENDING (needs PresentMon;
  foreground-fullscreen game detection works)      shows '--' until phase 2)
"""
from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import time

import psutil

_HERE = os.path.dirname(os.path.abspath(__file__))

try:
    import win32pdh  # noqa: F401  (pywin32, preinstalled) - reserved for fast PDH path
    _HAS_PDH = True
except Exception:
    _HAS_PDH = False

_NVIDIA_SMI = shutil.which("nvidia-smi")

# First call to cpu_percent always returns 0.0 -> prime once at import.
psutil.cpu_percent(interval=None)


# ---------------------------------------------------------------- CPU / RAM
def cpu_total() -> float:
    return float(psutil.cpu_percent(interval=None))


def cpu_per_core() -> list[float]:
    return [float(x) for x in psutil.cpu_percent(interval=None, percpu=True)]


def cpu_freq_mhz() -> float | None:
    try:
        f = psutil.cpu_freq()
        return float(f.current) if f else None
    except Exception:
        return None


def ram_info() -> dict:
    vm = psutil.virtual_memory()
    return {
        "percent": float(vm.percent),
        "used_gb": round(vm.used / 1024**3, 1),
        "total_gb": round(vm.total / 1024**3, 1),
    }


# ---------------------------------------------------------------- dGPU (NVIDIA)
_SMI_QUERY = "--query-gpu=utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,power.limit --format=csv,noheader,nounits"


def nvidia_info() -> dict | None:
    """Returns {util, vram_used_mb, vram_total_mb, temp_c, power_w} or None."""
    if not _NVIDIA_SMI:
        return None
    try:
        out = subprocess.check_output(
            f'nvidia-smi {_SMI_QUERY}', shell=True, text=True,
            stderr=subprocess.DEVNULL, timeout=5,
        ).strip().splitlines()
        if not out:
            return None
        parts = [p.strip() for p in out[0].split(",")]
        if len(parts) < 6:
            return None

        def _f(s: str) -> float | None:
            try:
                return float(s)
            except Exception:
                return None  # e.g. '[N/A]' when driver idle

        return {
            "util": _f(parts[0]),
            "vram_used_mb": _f(parts[1]),
            "vram_total_mb": _f(parts[2]),
            "temp_c": _f(parts[3]),
            "power_w": _f(parts[4]),
            # power.limit is [N/A] on this driver; current limit (follows
            # Legion Quiet/Balanced/Performance) comes from -q -d POWER.
            "power_limit_w": _f(parts[5]) or _smi_power_limit(),
        }
    except Exception:
        return None


_plimit = {"v": None, "ts": 0.0}


def _smi_power_limit() -> float | None:
    """Current GPU power limit via nvidia-smi -q (cached 5s)."""
    import re as _re
    import time as _t
    if _t.time() - _plimit["ts"] < 5 and _plimit["v"] is not None:
        return _plimit["v"]
    if not _NVIDIA_SMI:
        return None
    try:
        out = subprocess.check_output(
            "nvidia-smi -q -d POWER", shell=True, text=True,
            stderr=subprocess.DEVNULL, timeout=8)
        m = _re.search(r"Current Power Limit\s*:\s*([\d.]+)", out)
        if m:
            _plimit["v"] = float(m.group(1))
            _plimit["ts"] = _t.time()
            return _plimit["v"]
    except Exception:
        pass
    return _plimit["v"]


# ---------------------------------------------------------------- iGPU (optional)
def gpu_engines_by_phys() -> dict[str, float]:
    """Aggregate WMI GPUEngine UtilizationPercentage per phys_N adapter.

    phys mapping is adapter order (0 = usually dGPU, 1+ = iGPU). Returns e.g.
    {'phys_0': 0.4}. Empty dict on failure. Uses a single CIM query (~fast).
    """
    try:
        import win32com.client  # part of pywin32
        wmi = win32com.client.GetObject("winmgmts:\\\\.\\root\\cimv2")
        # NOTE: Win32_PerfFormattedData_* lives under root\\cimv2 on Win10/11.
        q = wmi.ExecQuery(
            "SELECT Name, UtilizationPercentage "
            "FROM Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine"
        )
        agg: dict[str, float] = {}
        for row in q:
            m = re.search(r"phys_(\d+)", row.Name or "")
            if not m:
                continue
            key = f"phys_{m.group(1)}"
            try:
                agg[key] = agg.get(key, 0.0) + float(row.UtilizationPercentage or 0)
            except Exception:
                pass
        return agg
    except Exception:
        return {}


def igpu_util() -> float | None:
    """Best-effort iGPU load: sum of non-phys_0 adapters, else None if absent."""
    agg = gpu_engines_by_phys()
    if not agg:
        return None
    others = [v for k, v in agg.items() if k != "phys_0"]
    if not others:
        return 0.0  # single-adapter enumeration -> iGPU idle/absent
    return float(sum(others))


# ------------------------------------------------- Legion GameZone WMI probe
# Lenovo exposes thermal/power modes + OC data under root\WMI (admin only).
# Probed once per process; snapshot() carries the cached dict for the UI/log.
_Lenovo = {"done": False, "data": {}}


def lenovo_probe() -> dict:
    """Read LENOVO_GAMEZONE_* data classes; {} when denied/unavailable."""
    try:
        import win32com.client
        wmi = win32com.client.GetObject("winmgmts:\\\\.\\root\\WMI")
        out: dict = {}
        for cls in ("LENOVO_GAMEZONE_DATA", "LENOVO_GAMEZONE_CPU_OC_DATA",
                    "LENOVO_GAMEZONE_GPU_OC_DATA"):
            try:
                for inst in wmi.ExecQuery(f"SELECT * FROM {cls}"):
                    d: dict = {}
                    try:
                        for p in inst.Properties_:
                            try:
                                v = p.Value
                                d[p.Name] = list(v) if isinstance(v, tuple) else v
                            except Exception:
                                pass
                    except Exception:
                        pass
                    out[cls] = d
            except Exception:
                pass
        return out
    except Exception:
        return {}


# ------------------------------------------------- vendor detection (icon colors)
_cpu_vendor: str | None = None
_gpu_vendor: str | None = None
_hz: int | None = None


def cpu_vendor() -> str:
    """'intel' | 'amd' | 'unknown' (from PROCESSOR_IDENTIFIER, instant)."""
    global _cpu_vendor
    if _cpu_vendor is None:
        ident = os.environ.get("PROCESSOR_IDENTIFIER", "").lower()
        _cpu_vendor = "amd" if "authenticamd" in ident else "intel" if "genuineintel" in ident else "unknown"
    return _cpu_vendor


def gpu_vendor() -> str:
    """'nvidia' | 'amd' | 'intel' | 'unknown' (cached)."""
    global _gpu_vendor
    if _gpu_vendor is None:
        _gpu_vendor = "unknown"
        if _NVIDIA_SMI:
            _gpu_vendor = "nvidia"
        else:
            try:
                import win32com.client
                wmi = win32com.client.GetObject("winmgmts:\\\\.\\root\\cimv2")
                names = " ".join(getattr(x, "Name", "") or "" for x in
                                 wmi.ExecQuery("SELECT Name FROM Win32_VideoController")).lower()
                if "radeon" in names or " amd " in f" {names} ":
                    _gpu_vendor = "amd"
                elif "intel" in names:
                    _gpu_vendor = "intel"
                elif "nvidia" in names or "geforce" in names:
                    _gpu_vendor = "nvidia"
            except Exception:
                pass
    return _gpu_vendor


def display_hz() -> int:
    """Primary display refresh rate (for FPS normalization); cached, fallback 144."""
    global _hz
    if _hz is None:
        _hz = 144
        try:
            import win32com.client
            wmi = win32com.client.GetObject("winmgmts:\\\\.\\root\\cimv2")
            for x in wmi.ExecQuery("SELECT CurrentRefreshRate FROM Win32_VideoController"):
                try:
                    if x.CurrentRefreshRate and int(x.CurrentRefreshRate) > 0:
                        _hz = int(x.CurrentRefreshRate)
                        break
                except Exception:
                    continue
        except Exception:
            pass
    return _hz


# ------------------------------------------- CPU temp/watt (isolated bridge)
# The LibreHardwareMonitor reader runs in vendor/lhm_bridge.py as a CHILD
# PROCESS: pythonnet can die with an unhandled CLR exception that would
# otherwise take down the whole monitor with no traceback. The bridge writes
# vendor/lhm.json every ~2s; here we only supervise it and read the file.
_BR = {"proc": None}


def _bridge_stop():
    p, _BR["proc"] = _BR["proc"], None
    try:
        if p is not None and p.poll() is None:
            p.terminate()
            try:
                p.wait(timeout=3)
            except Exception:
                p.kill()
    except Exception:
        pass


def _bridge_ensure():
    p = _BR["proc"]
    if p is not None and p.poll() is None:
        return
    _BR["proc"] = None
    try:
        bridge = os.path.join(_HERE, "vendor", "lhm_bridge.py")
        if not os.path.exists(bridge):
            return
        si = subprocess.STARTUPINFO()
        si.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        _BR["proc"] = subprocess.Popen(
            [sys.executable, bridge],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            startupinfo=si, creationflags=0x08000000)  # CREATE_NO_WINDOW
    except Exception:
        _BR["proc"] = None


def _embedded_cpu() -> tuple:
    """Returns (temp_c, watts) from the bridge's JSON, or (None, None)."""
    _bridge_ensure()
    try:
        with open(os.path.join(_HERE, "vendor", "lhm.json")) as f:
            d = json.load(f)
        if time.time() - d.get("ts", 0) > 10:
            return None, None
        return d.get("temp"), d.get("watts")
    except Exception:
        return None, None


def lhm_sensors() -> dict:
    """Fallback: LibreHardwareMonitor/OpenHardwareMonitor WMI (separate app)."""
    try:
        import win32com.client
        for ns in (r"root\LibreHardwareMonitor", r"root\OpenHardwareMonitor"):
            try:
                wmi = win32com.client.GetObject(f"winmgmts:\\\\.\\{ns}")
                out: dict = {}
                for s in wmi.ExecQuery("SELECT Name, SensorType, Value FROM Sensor"):
                    try:
                        out.setdefault(f"{s.SensorType}/{s.Name}", float(s.Value))
                    except Exception:
                        pass
                if out:
                    return out
            except Exception:
                continue
    except Exception:
        pass
    return {}


def cpu_temp_watt() -> dict:
    """Returns {temp_c, watts}; embedded lib first, WMI fallback, else Nones."""
    temp, watts = _embedded_cpu()
    if temp is None and watts is None:
        s = lhm_sensors()
        for k, v in s.items():
            kl = k.lower()
            if "temperature" in kl and ("package" in kl or "cpu" in kl) and temp is None:
                temp = v
            if "power" in kl and ("package" in kl or "cpu" in kl) and watts is None:
                watts = v
    return {"temp_c": temp, "watts": watts}


# ---------------------------------------------------------------- FPS (phase 2)
_GAME_EXES = {
    "cs2.exe", "valorant.exe", "fortniteclient-win64-shipping.exe",
    "cod.exe", "cyberpunk2077.exe", "eldenring.exe", "minecraft.exe",
    "rdr2.exe", "apex_legends.exe", "overwatch.exe", "baldurs_gate3.exe",
    "b1.exe", "sekiro.exe", "dsr.exe", "witcher3.exe",
    "limbo.exe", "portal2.exe", "hl2.exe", "nightreign.exe",
    "darksoulsiii.exe", "darksoulsremastered.exe", "hollow_knight.exe",
    "terraria.exe", "stardewvalley.exe", "celeste.exe", "csgo.exe",
}


def running_game() -> str | None:
    """Return a game exe name if any known game process is running (even background)."""
    try:
        for p in psutil.process_iter(["name"]):
            nm = (p.info.get("name") or "")
            if nm.lower() in _GAME_EXES:
                return nm
    except Exception:
        pass
    return None


def foreground_game() -> str | None:
    """Return foreground process name if it looks like a game.

    Heuristic: fullscreen window + not a system shell process. This catches
    lightweight 2D games (Limbo sips ~2% GPU) that a GPU-load gate would miss.
    Real per-second FPS comes from PresentMon (needs elevation).
    """
    _SHELL_EXES = {
        "explorer.exe", "dwm.exe", "taskmgr.exe", "shellexperiencehost.exe",
        "startmenuexperiencehost.exe", "searchhost.exe", "textinputhost.exe",
        "lockapp.exe", "taskhostw.exe", "sihost.exe", "useroobebroker.exe",
        "snippingtool.exe", "snipandsketch.exe", "screenclippinghost.exe",
        "screencapturehost.exe",
    }
    try:
        import ctypes
        from ctypes import wintypes
        hwnd = ctypes.windll.user32.GetForegroundWindow()
        if not hwnd:
            return None
        rect = wintypes.RECT()
        ctypes.windll.user32.GetWindowRect(hwnd, ctypes.byref(rect))
        sw = ctypes.windll.user32.GetSystemMetrics(0)
        sh = ctypes.windll.user32.GetSystemMetrics(1)
        fullscreen = (rect.right - rect.left >= sw - 2) and (rect.bottom - rect.top >= sh - 2)
        if not fullscreen:
            return None
        pid = wintypes.DWORD()
        ctypes.windll.user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        name = psutil.Process(pid.value).name()
        if name.lower() in _SHELL_EXES:
            return None
        return name
    except Exception:
        return None


# ------------------------------------------------- FPS via PresentMon (real!)
# PresentMon needs elevation (ETW trace session) -> without admin, capture
# fails to start and fps stays None. Final app runs elevated via logon task.
# Frames stream over the child's STDOUT pipe (the CSV file output can't be
# tailed reliably: the writer locks it), parsed by a reader thread into a
# rolling deque; fps_poll() derives per-second FPS from fresh frames.
_PM = {"proc": None, "game": None, "gen": 0,
       "frames": None, "cols": None}

import collections as _collections


def _pm_exe() -> str | None:
    p = os.path.join(_HERE, "vendor", "PresentMon", "PresentMon.exe")
    return p if os.path.exists(p) else None


def _pm_stop():
    _PM["gen"] += 1
    p, _PM["proc"] = _PM["proc"], None
    _PM["game"] = None
    try:
        if p is not None:
            try:
                if p.stdout:
                    p.stdout.close()
            except Exception:
                pass
            if p.poll() is None:
                p.terminate()
                try:
                    p.wait(timeout=3)
                except Exception:
                    p.kill()
    except Exception:
        pass


def _pm_reader(gen: int):
    """Consume the capture's CSV stdout into a rolling frame deque."""
    proc = _PM["proc"]
    try:
        mi = ti = -1
        for line in proc.stdout:
            if _PM["gen"] != gen:
                break
            line = line.strip()
            if not line:
                continue
            parts = line.split(",")
            if mi < 0:
                header = [h.strip() for h in parts]
                if "MsBetweenPresents" in header and "TimeInSeconds" in header:
                    mi = header.index("MsBetweenPresents")
                    ti = header.index("TimeInSeconds")
                continue
            if len(parts) <= max(mi, ti):
                continue
            try:
                _PM["frames"].append((float(parts[ti]), float(parts[mi])))
            except Exception:
                pass
    except Exception:
        pass


def _pm_start(game_exe: str):
    _pm_stop()
    exe = _pm_exe()
    if not exe:
        return
    try:
        si = subprocess.STARTUPINFO()
        si.dwFlags |= subprocess.STARTF_USESHOWWINDOW
        proc = subprocess.Popen(
            [exe, "--process_name", game_exe, "--output_stdout",
             "--v1_metrics", "--no_console_stats", "--stop_existing_session"],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            startupinfo=si, creationflags=0x08000000,  # CREATE_NO_WINDOW
            text=True, bufsize=1)
        _PM["proc"] = proc
        _PM["game"] = game_exe
        _PM["frames"] = _collections.deque(maxlen=400)
        gen = _PM["gen"]
        import threading as _th
        _th.Thread(target=_pm_reader, args=(gen,), daemon=True).start()
    except Exception:
        _PM["proc"] = None
        _PM["game"] = None


def fps_poll(game_exe: str | None):
    """Start/stop capture around the detected game; return live FPS or None."""
    import time as _t
    if not game_exe:
        if _PM["proc"] is not None or _PM["game"] is not None:
            _pm_stop()
        return None
    proc = _PM["proc"]
    if _PM["game"] != game_exe or (proc is not None and proc.poll() is not None):
        if _t.time() - _PM.get("last_try", 0) > 5:  # don't respawn-churn a dying capture
            _PM["last_try"] = _t.time()
            _pm_start(game_exe)  # (re)start -> warming up, no number yet
        return None
    frames = _PM["frames"] or []
    if len(frames) < 5:
        return None
    now = frames[-1][0]
    recent = [m for t, m in frames if now - t <= 2.0 and m > 0]
    if len(recent) < 5:
        return None
    return round(1000.0 / (sum(recent) / len(recent)), 1)


import atexit as _atexit
_atexit.register(_pm_stop)
_atexit.register(_bridge_stop)


# ------------------------------------------------- disk/net throughput (MB/s)
_io = {"disk": None, "net": None, "ts": 0.0}


def io_rates() -> dict:
    """Per-second disk total + net down/up in MB/s (first call returns zeros)."""
    out = {"disk": 0.0, "down": 0.0, "up": 0.0}
    try:
        d = psutil.disk_io_counters()
        n = psutil.net_io_counters()
        if _io["disk"] is not None:
            dt = max(0.2, time.time() - _io["ts"])
            out["disk"] = ((d.read_bytes - _io["disk"][0]) + (d.write_bytes - _io["disk"][1])) / dt / 1048576
            out["down"] = (n.bytes_recv - _io["net"][0]) / dt / 1048576
            out["up"] = (n.bytes_sent - _io["net"][1]) / dt / 1048576
        _io["disk"] = (d.read_bytes, d.write_bytes)
        _io["net"] = (n.bytes_recv, n.bytes_sent)
        _io["ts"] = time.time()
    except Exception:
        pass
    return out


# ---------------------------------------------------------------- snapshot
def snapshot() -> dict:
    nv = nvidia_info() or {}
    ct = cpu_temp_watt()
    game = foreground_game() or running_game()
    io = io_rates()
    if not _Lenovo["done"]:
        _Lenovo["done"] = True
        _Lenovo["data"] = lenovo_probe()
    return {
        "ts": time.time(),
        "cpu": cpu_total(),
        "cores": cpu_per_core(),
        "freq_mhz": cpu_freq_mhz(),
        "ram": ram_info(),
        "gpu_util": nv.get("util"),
        "vram_used_mb": nv.get("vram_used_mb"),
        "vram_total_mb": nv.get("vram_total_mb"),
        "gpu_temp": nv.get("temp_c"),
        "gpu_watts": nv.get("power_w"),
        "gpu_limit_w": nv.get("power_limit_w"),
        "igpu": igpu_util(),
        "cpu_temp": ct["temp_c"],
        "cpu_watts": ct["watts"],
        "game": game,
        "fps": fps_poll(game),
        "lenovo": _Lenovo["data"],
        "disk_mbs": io["disk"],
        "net_down": io["down"],
        "net_up": io["up"],
    }


if __name__ == "__main__":
    import json
    time.sleep(1.2)  # let psutil deltas settle
    print(json.dumps(snapshot(), indent=1))
