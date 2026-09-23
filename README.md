# Resource Monitor (draft v0.1)

Minimal always-on-top Windows monitor. Upper-left pills, transparency,
tray background mode, per-core grid (green→red).

## Run draft
```bat
cd development\resource-monitor
pip install -r requirements.txt
python app.py
```

Controls: drag = move · double-click / C = core grid · H = hide to tray ·
tray icon = Show / Cores / Quit (runs in background like Discord/NV panel).

## Metric status (verified 2026-09-22, RTX 5070 Ti Laptop + Ultra 9 275HX)
| Metric | Source | Status |
|---|---|---|
| CPU % + per-core + freq | psutil | ✅ live |
| RAM % + GB | psutil | ✅ live |
| GPU util + VRAM + temp + watts | nvidia-smi | ✅ live |
| iGPU % | WMI GPUEngine | ✅ live (optional) |
| CPU temp + package watts | LibreHardwareMonitor | ⏳ phase 2 (shows --) |
| FPS in game | PresentMon | ⏳ phase 2 (shows --, game detect ready) |

## Next (phase 2)
1. Bundle LibreHardwareMonitorLib via pythonnet → CPU temp/watts, no extra app.
2. PresentMon session → real FPS pill when fullscreen game detected.
3. PyInstaller one-file exe + Inno Setup installer + autostart + settings
   (opacity, position, polling rate, °C/°F).
