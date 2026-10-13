//! NVIDIA dGPU via NVML (loaded at runtime; absent on non-NVIDIA systems).
//!
//! Replaces the Python version's `nvidia-smi` process spawn every second
//! (plus a second `nvidia-smi -q` for the power limit).
use std::ffi::c_void;
use std::time::{Duration, Instant};

use libloading::Library;

use super::Gpu;

type Dev = *mut c_void;
type FInit = unsafe extern "C" fn() -> i32;
type FHandle = unsafe extern "C" fn(u32, *mut Dev) -> i32;
type FUtil = unsafe extern "C" fn(Dev, *mut [u32; 2]) -> i32;
type FMem1 = unsafe extern "C" fn(Dev, *mut [u64; 3]) -> i32;
type FMem2 = unsafe extern "C" fn(Dev, *mut Mem2) -> i32;
type FU32 = unsafe extern "C" fn(Dev, *mut u32) -> i32;
type FTemp = unsafe extern "C" fn(Dev, u32, *mut u32) -> i32;

#[repr(C)]
#[derive(Default)]
struct Mem2 {
    version: u32,
    total: u64,
    reserved: u64,
    free: u64,
    used: u64,
}

pub struct Nvml {
    _lib: Library,
    dev: Dev,
    util: FUtil,
    mem1: FMem1,
    mem2: Option<FMem2>,
    temp: FTemp,
    power: FU32,
    limit: FU32,
    limit_cache: std::cell::Cell<(Option<f32>, Option<Instant>)>,
}

// The device handle is only used from the sampler thread.
unsafe impl Send for Nvml {}

impl Nvml {
    pub fn load() -> Option<Self> {
        let candidates = [
            "nvml.dll".to_string(),
            r"C:\Program Files\NVIDIA Corporation\NVSMI\nvml.dll".to_string(),
        ];
        let lib = candidates.iter().find_map(|p| unsafe { Library::new(p).ok() })?;
        unsafe {
            let init: FInit = *lib.get::<FInit>(b"nvmlInit_v2\0").ok()?;
            if init() != 0 {
                return None;
            }
            let handle: FHandle = *lib.get::<FHandle>(b"nvmlDeviceGetHandleByIndex_v2\0").ok()?;
            let mut dev: Dev = std::ptr::null_mut();
            if handle(0, &mut dev) != 0 {
                return None;
            }
            Some(Self {
                util: *lib.get(b"nvmlDeviceGetUtilizationRates\0").ok()?,
                mem1: *lib.get(b"nvmlDeviceGetMemoryInfo\0").ok()?,
                mem2: lib.get::<FMem2>(b"nvmlDeviceGetMemoryInfo_v2\0").ok().map(|s| *s),
                temp: *lib.get(b"nvmlDeviceGetTemperature\0").ok()?,
                power: *lib.get(b"nvmlDeviceGetPowerUsage\0").ok()?,
                limit: *lib.get(b"nvmlDeviceGetEnforcedPowerLimit\0").ok()?,
                dev,
                _lib: lib,
                limit_cache: std::cell::Cell::new((None, None)),
            })
        }
    }

    pub fn sample(&self) -> Gpu {
        let mut g = Gpu::default();
        unsafe {
            let mut u = [0u32; 2];
            if (self.util)(self.dev, &mut u) == 0 {
                g.util = Some(u[0] as f32);
            }
            // v2 "used" excludes driver-reserved memory, matching nvidia-smi.
            let mut m2 = Mem2 { version: std::mem::size_of::<Mem2>() as u32 | (2 << 24), ..Default::default() };
            let mut m1 = [0u64; 3];
            if self.mem2.is_some_and(|f| f(self.dev, &mut m2) == 0) {
                g.vram_used_mb = Some(m2.used as f32 / 1048576.0);
                g.vram_total_mb = Some(m2.total as f32 / 1048576.0);
            } else if (self.mem1)(self.dev, &mut m1) == 0 {
                g.vram_used_mb = Some(m1[2] as f32 / 1048576.0);
                g.vram_total_mb = Some(m1[0] as f32 / 1048576.0);
            }
            let mut t = 0u32;
            if (self.temp)(self.dev, 0, &mut t) == 0 {
                g.temp = Some(t as f32);
            }
            let mut mw = 0u32;
            if (self.power)(self.dev, &mut mw) == 0 {
                g.watts = Some(mw as f32 / 1000.0);
            }
            // Enforced limit follows the laptop's power mode; refresh every 5s.
            let (cached, at) = self.limit_cache.get();
            if at.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
                let mut lw = 0u32;
                let v = if (self.limit)(self.dev, &mut lw) == 0 && lw > 0 { Some(lw as f32 / 1000.0) } else { cached };
                self.limit_cache.set((v, Some(Instant::now())));
                g.limit_w = v;
            } else {
                g.limit_w = cached;
            }
        }
        g
    }
}
