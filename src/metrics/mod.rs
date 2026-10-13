//! Metrics collection on one background thread, 1 Hz.
//!
//! Sources (all in-process except the two optional elevated helpers):
//! - CPU total/per-core ......... NtQuerySystemInformation deltas
//! - RAM ........................ GlobalMemoryStatusEx
//! - NVIDIA dGPU ................ NVML (nvml.dll), no nvidia-smi process spawns
//! - iGPU / non-NVIDIA dGPU ..... PDH "GPU Engine", grouped per adapter LUID (DXGI)
//! - disk ....................... PDH PhysicalDisk(_Total); per-drive usage + physical disk (IOCTL)
//! - network .................... GetIfTable2, physical adapters only
//! - CPU temp / package W ....... ACPI thermal zone + RAPL Energy Meter (PDH, no admin);
//!   (LibreHardwareMonitor bridge when elevated)
//! - FPS ........................ PresentMon child process (needs elevation)
mod cpu;
mod disks;
mod game;
mod gpu;
pub mod jobs;
mod lhm;
mod net;
mod nvml;
mod pdh;
mod presentmon;

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

pub use disks::Drive;
pub use gpu::Vendor;

#[derive(Clone, Debug, Default)]
pub struct Gpu {
    pub util: Option<f32>,
    pub vram_used_mb: Option<f32>,
    pub vram_total_mb: Option<f32>,
    pub temp: Option<f32>,
    pub watts: Option<f32>,
    pub limit_w: Option<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub cpu: f32,
    pub cores: Vec<f32>,
    pub ram_pct: f32,
    pub ram_used_gb: f32,
    pub ram_total_gb: f32,
    pub gpu: Gpu,
    pub igpu: Option<f32>,
    pub cpu_temp: Option<f32>,
    pub cpu_watts: Option<f32>,
    pub game: Option<String>,
    pub fps: Option<f32>,
    pub hz: u32,
    pub disk_mbs: f32,
    pub drives: Vec<Drive>,
    pub net_down: f32,
    pub net_up: f32,
}

/// Static facts the UI needs before the first sample (icon colors, core grid).
#[derive(Clone, Copy)]
pub struct Info {
    pub cores: usize,
    pub cpu_vendor: Vendor,
    pub gpu_vendor: Vendor,
}

static LATEST: Mutex<Option<Snapshot>> = Mutex::new(None);
static HWND_TARGET: AtomicIsize = AtomicIsize::new(0);
static STOP: AtomicBool = AtomicBool::new(false);

pub fn take_latest() -> Option<Snapshot> {
    LATEST.lock().ok()?.take()
}

pub fn info() -> Info {
    Info {
        cores: cpu::core_count().max(1),
        cpu_vendor: gpu::cpu_vendor(),
        gpu_vendor: gpu::dgpu_vendor(),
    }
}

/// Spawn the sampler; it posts `msg` to `hwnd` whenever a fresh snapshot is ready.
pub fn start(hwnd: HWND, msg: u32) {
    HWND_TARGET.store(hwnd.0 as isize, Ordering::SeqCst);
    std::thread::Builder::new()
        .name("sampler".into())
        .spawn(move || sampler(msg))
        .expect("sampler thread");
}

pub fn stop() {
    STOP.store(true, Ordering::SeqCst);
    presentmon::stop();
    lhm::stop();
}

fn sampler(msg: u32) {
    let mut cpu = cpu::Cpu::new();
    let nv = nvml::Nvml::load();
    crate::log::log(&format!("nvml: {}", if nv.is_some() { "ok" } else { "unavailable" }));
    let adapters = gpu::adapters();
    let mut pdh = pdh::Pdh::new(&adapters, nv.is_none());
    let mut net = net::Net::new();
    let mut disks = disks::Disks::new();
    let mut games = game::Detector::new();
    let elevated = jobs::is_elevated();
    crate::log::log(&format!(
        "start: cores={} adapters={:?} elevated={}",
        cpu::core_count(),
        adapters.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
        elevated
    ));
    let mut hz = gpu::display_hz();
    let mut hz_at = Instant::now();
    let mut n: u64 = 0;

    // CPU/PDH/net deltas need a baseline: prime, then first real sample 1s later.
    std::thread::sleep(Duration::from_millis(1000));
    while !STOP.load(Ordering::SeqCst) {
        let t0 = Instant::now();
        n += 1;
        let (cpu_total, cores) = cpu.sample();
        let (ram_pct, ram_used_gb, ram_total_gb) = cpu::ram();
        let p = pdh.sample();
        let mut g = nv.as_ref().map(|nv| nv.sample()).unwrap_or_default();
        if nv.is_none() {
            g.util = p.dgpu;
            g.vram_used_mb = p.dgpu_vram_mb;
            g.vram_total_mb = adapters.iter().find(|a| a.is_dgpu).map(|a| a.dedicated_mb);
        }
        let (down, up) = net.sample();
        let scan = games.tick();
        lhm::ensure(elevated || scan.lhm_app_running);
        // LHM (elevated, exact Tctl/package sensors) wins; built-in Windows sensors otherwise.
        let (lt, lw) = lhm::read();
        let (cpu_temp, cpu_watts) = (lt.or(p.cpu_temp), lw.or(p.cpu_watts));
        let fps = if elevated { presentmon::poll(scan.game.as_deref()) } else { None };
        if hz_at.elapsed() > Duration::from_secs(10) {
            hz = gpu::display_hz(); // laptops switch refresh rate with power mode
            hz_at = Instant::now();
        }
        let snap = Snapshot {
            cpu: cpu_total,
            cores,
            ram_pct,
            ram_used_gb,
            ram_total_gb,
            gpu: g,
            igpu: p.igpu,
            cpu_temp,
            cpu_watts,
            game: scan.game,
            fps,
            hz: scan.hz.unwrap_or(hz),
            disk_mbs: p.disk_mbs,
            drives: disks.sample(),
            net_down: down,
            net_up: up,
        };
        if n <= 3 {
            crate::log::log(&format!("tick {n} took {:?}: {snap:?}", t0.elapsed()));
        }
        if let Ok(mut l) = LATEST.lock() {
            *l = Some(snap);
        }
        let h = HWND(HWND_TARGET.load(Ordering::SeqCst) as *mut _);
        unsafe {
            let _ = PostMessageW(Some(h), msg, WPARAM(0), LPARAM(0));
        }
        let spent = t0.elapsed();
        std::thread::sleep(Duration::from_millis(1000).saturating_sub(spent).max(Duration::from_millis(200)));
    }
}
