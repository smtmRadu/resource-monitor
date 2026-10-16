# Resource Monitor

Always-on-top, click-through, transparent pill monitor for Windows, docked in
the top-left corner of a display. Written in Rust (a rewrite of the earlier
Python/Tk version, with its bugs fixed and a fraction of its footprint).

[![Download for Windows](https://img.shields.io/badge/Download-Windows%20installer-2ea44f?style=for-the-badge&logo=windows)](https://github.com/smtmRadu/resource-monitor/raw/main/dist/ResourceMonitor-Setup.exe)

![Resource Monitor](docs/screenshot.png)

## Install
1. Download **[ResourceMonitor-Setup.exe](https://github.com/smtmRadu/resource-monitor/raw/main/dist/ResourceMonitor-Setup.exe)** (2 MB) and run it.
   The installer is not code-signed. If Windows SmartScreen warns, click **More info → Run anyway**.
2. Pick the install folder (default `C:\Program Files\Resource Monitor`).
   Keep **Enable on startup** checked to start the monitor at every logon,
   elevated without a UAC prompt. Elevation enables FPS and the exact CPU sensors.

The installer holds no app build. It downloads the latest one from this repo's
`main` branch, so it never goes stale. Uninstall it from **Settings → Apps**.

## Updates
The app checks `main` for a new version 1 minute after start, then every hour.
When offline, it retries every 10 minutes. It never updates by itself. When a new
version exists, the tray menu shows it as a green superscript next to the
current version (e.g. `Version 0.3.0 ↑0.3.1`).

Click the version item to update now. The monitor restarts in a few seconds,
with a UAC prompt if it is not running elevated. Click it when no arrow shows to
check immediately. Only changed files are downloaded, and each one is
hash-checked.

## What it shows
One row per module: an **icon pill**, a **metrics pill**, and a 60 s **history
ribbon**. A "snake" runs around each icon pill; its length is the live value.

| Row | Metrics pill | Snake length |
|---|---|---|
| CPU | load %, package watts, per-core grid (green → red), temperature ring | CPU load |
| RAM | load %, used / total GB | RAM load |
| GPU | load %, watts / power limit, temperature ring | GPU load |
| VRAM | used %, used / total GB | VRAM used |
| iGPU | load % (row appears on activity, hides after 5 s at 0%) | iGPU load |
| DISK | transfer MB/s + a storage bar per drive (color = physical disk; red under 10% free, like Explorer) | storage used, all drives |
| ETH | download / upload MB/s | two snakes on opposite sides (download blue, upload purple), each up to half the pill at 100 MB/s |
| FPS | game name + FPS, yellow above the refresh rate (row appears only while a game runs) | FPS / refresh rate of the game's monitor |

Temperature rings fill from 30 °C to 99 °C; above 99 °C they show a flame. They
ease to each new reading (arc, color and number together). Other values
crossfade over 0.3 s. The history ribbons scroll smoothly, with a fill under each line that fades toward 0%; the CPU and GPU
ribbons name the model, and the FPS ribbon shows its full scale (the refresh
rate; a faster game stays capped at the top line, the number shows the real FPS).
Rows that appear or hide (iGPU, FPS) collapse and expand smoothly.

**Game detection:** the focused app counts as a game when it covers its monitor
(fullscreen / borderless) or renders 3D (windowed, ≥ 5% of the GPU's 3D engine),
or when a known game exe runs. A detected game stays the game while unfocused,
until it exits. In **game mode** the pill backgrounds fade to 25% and text gets
a dark halo, so the overlay stays readable without hiding the game.

Games in **exclusive / optimized fullscreen** (e.g. Dark Souls III's
"Fullscreen") are drawn without the desktop compositor, so no normal window can
show above them. Use the game's **borderless** or **windowed** mode to see the
monitor while playing.

## Controls
The window is click-through, so everything lives in the **tray icon**
(right-click; left-click does nothing):

- **Version** – current version, plus the new one when available; click to check / update
- **Hide / Show**
- **Track** – history ribbons on/off
- **Mode** – **Overlayed** (floats over windows) or **Exclusive** (reserves a strip
  on the display's left edge, like the taskbar: maximized and snapped windows
  start right of it)
- **Size** – 1x / 1.5x / 2x
- **Display** – which monitor it docks on (only with 2+ displays)
- **Quit**

Choices persist in `%LOCALAPPDATA%\ResourceMonitor\settings.json` (dev builds:
the repo folder) and survive updates. First run: with one display it starts
Overlayed on it; with 2+ it starts Exclusive on the last one. The layout is
DPI-aware. Starting the app again (e.g. from the Start Menu) shows a hidden
monitor.

## Build & run
Requires the Rust toolchain (MSVC). Windows 10/11 only.

```bat
cargo build --release
target\release\resource-monitor-rs.exe
```

Only one instance runs at a time. Optional elevated autostart (adds FPS and the
exact LibreHardwareMonitor CPU sensors), from an **elevated** PowerShell:

```powershell
.\setup_task.ps1
```

Off-screen self-test (no window; renders frames to `<dir>\*.rgba` and timings to
`selftest.txt`, including synthetic game-mode / scale / track-off frames and the
tray version item):

```bat
target\release\resource-monitor-rs.exe --selftest <dir>
```

## Release
Installed copies update from `dist/` on `main`:

1. Bump `version` in `Cargo.toml`.
2. Run `.\release.ps1`. It builds the exe into `dist/` and writes
   `dist/manifest.json` (every app file with its repo path and sha256).
3. Commit `dist/` (and any changed `vendor/` files) and push to `main`.

`.\release.ps1 -Installer` also rebuilds `dist/ResourceMonitor-Setup.exe` with
Inno Setup 6 (`installer/setup.iss`). This is needed only when the installer itself
changes. `.gitattributes` turns off line-ending conversion for the shipped files,
so the checkout bytes match the manifest hashes.

To test a release before pushing it, serve the repo locally
(`python -m http.server 8765`) and set `RM_UPDATE_BASE=http://127.0.0.1:8765/`.
The updater then reads from there. `--update` brings an install folder up to date
(the installer runs it).

## Metric sources
Everything is read in-process with Windows APIs; no admin and no extra apps
are needed except where noted.

| Metric | Source | Admin |
|---|---|---|
| CPU % total + per core | `NtQuerySystemInformation` | no |
| CPU temperature | ACPI thermal zone (PDH `Thermal Zone Information`); the zone that actually moves is picked automatically | no |
| CPU package watts | RAPL (PDH `Energy Meter`, `RAPL_Package0_PKG`) | no |
| RAM | `GlobalMemoryStatusEx` | no |
| NVIDIA GPU load / VRAM / temp / watts / limit | NVML (`nvml.dll`, loaded at runtime) | no |
| iGPU %, non-NVIDIA dGPU % | PDH `GPU Engine`, per adapter LUID (adapters via D3DKMT) | no |
| Disk MB/s | PDH `PhysicalDisk(_Total)` | no |
| Drive usage + physical disk | `GetDiskFreeSpaceExW` (every 5 s) + `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` | no |
| Network MB/s | `GetIfEntry2` on physical NICs only | no |
| Game detection | foreground window covering its monitor or using the 3D engine (PDH `GPU Engine`, per process), or a known game exe | no |
| FPS | PresentMon child process (`vendor/PresentMon`), game's main swap chain | **yes** |
| Exact CPU sensors (Tctl / package) | LibreHardwareMonitor via `vendor/lhm_bridge.ps1`; preferred when available | **yes** (or LHM app running) |

Helper processes run in a kill-on-close job object, so they never outlive the
monitor.

## Footprint
Measured with v0.3.1 on a Core Ultra 9 275HX + RTX 5070 Ti Laptop (24 threads),
1x, all rows and animations, elevated (FPS + LHM sensors), over 60 s:

| | |
|---|---|
| CPU | **2.9% of one core** (0.12% of the whole CPU) |
| GPU | 0% |
| Memory | **55 MB** working set, 35 MB private (NVIDIA's NVML is a good part of it) |
| Disk I/O | 0 KB |
| Threads / GDI objects | 14 / 13 |
| Executable | 900 KB |

How it stays light:
- Metrics are sampled once per second on one background thread (~0.2 ms per tick).
- The static layer (pills, text, ribbon backgrounds) is redrawn once per second.
- Ribbon lines, easing rings and the values crossfade are composited onto a
  cached copy of that layer at 30 fps (~0.1 ms per frame); a settled ring is
  baked back into the static layer.
- The snakes live on a small separate overlay window over the icon column and
  are redrawn at up to 60 fps (the monitor refresh rate on slower panels),
  paced by `DwmFlush`. Pacing stops when nothing moves or the monitor is hidden.
- Adapter discovery uses D3DKMT instead of DXGI, which would wake a sleeping
  NVIDIA GPU on hybrid laptops (2 W → 17 W).
- The update check is one ~13 KB request per hour. Installed files are hashed
  once, then cached by size and modified time.

## Fixed vs the Python version
- iGPU never showed: adapters were told apart by PDH `phys_N`, which is
  `phys_0` for both GPUs on hybrid laptops. Now per adapter LUID, using the
  busiest engine (no >100% overshoot).
- CPU temperature never read on AMD (the bridge only knew Intel's "Package"
  sensor) and never without admin. Now the ACPI thermal zone + RAPL, no admin.
- Dead controls: double-click / C / H / right-click bindings could never fire on
  a click-through window, and "hide to tray" had no way back.
- Rows reordered after toggling (the iGPU row came back under ETH; Track off/on
  moved every ribbon to the end).
- Game mode removed the pill backgrounds entirely instead of fading them.
- Fullscreen browsers / video players and maximized windows counted as games.
- FPS froze on its last value when a game stopped presenting frames, and mixed
  in launcher / overlay swap chains.
- Network double-counted WSL / Hyper-V virtual adapter traffic and spiked when
  an adapter reconnected.
- `app.log` grew by a line every second forever.
- The helper respawned every second when it could not run, and helpers were
  left running after the monitor was killed.
- Several instances could run at once; blurry on scaled displays; the autostart
  script had hardcoded user paths.
