//! PDH counters: per-adapter GPU engine load, dedicated VRAM, disk throughput,
//! and the built-in (no admin, no third-party app) CPU sensors:
//! - CPU package power: "Energy Meter" RAPL instances (Windows EMI, milliwatts)
//! - CPU temperature: the ACPI thermal zone that actually moves (zones that sit
//!   at a fixed trip value or read 0 K are ignored)
use std::collections::HashMap;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::System::Performance::*;

use super::gpu::Adapter;

const PDH_MORE_DATA: u32 = 0x800007D2;

pub struct Sample {
    pub igpu: Option<f32>,
    pub dgpu: Option<f32>,
    pub dgpu_vram_mb: Option<f32>,
    pub disk_mbs: f32,
    pub cpu_temp: Option<f32>,
    pub cpu_watts: Option<f32>,
}

pub struct Pdh {
    query: PDH_HQUERY,
    disk: Option<PDH_HCOUNTER>,
    engines: Option<PDH_HCOUNTER>,
    vram: Option<PDH_HCOUNTER>,
    igpu_luid: Option<String>,
    dgpu_luid: Option<String>,
    thermal: Option<PDH_HCOUNTER>,
    energy: Option<PDH_HCOUNTER>,
    /// Per thermal zone: distinct readings seen (tenths of K, capped list).
    zones: HashMap<String, Vec<i64>>,
}

// PDH handles are used from the sampler thread only.
unsafe impl Send for Pdh {}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

impl Pdh {
    /// `need_dgpu`: also derive dGPU load/VRAM here (no NVML on this system).
    pub fn new(adapters: &[Adapter], need_dgpu: bool) -> Self {
        let igpu_luid = adapters.iter().find(|a| a.is_igpu).map(|a| a.luid.clone());
        let dgpu_luid = if need_dgpu { adapters.iter().find(|a| a.is_dgpu).map(|a| a.luid.clone()) } else { None };
        let mut s = Self {
            query: PDH_HQUERY::default(),
            disk: None,
            engines: None,
            vram: None,
            igpu_luid,
            dgpu_luid,
            thermal: None,
            energy: None,
            zones: HashMap::new(),
        };
        unsafe {
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut s.query) != 0 {
                return s;
            }
            s.disk = s.add(r"\PhysicalDisk(_Total)\Disk Bytes/sec");
            // Engine counters enumerate every process: only pay for them when needed.
            if s.igpu_luid.is_some() || s.dgpu_luid.is_some() {
                s.engines = s.add(r"\GPU Engine(*)\Utilization Percentage");
            }
            if s.dgpu_luid.is_some() {
                s.vram = s.add(r"\GPU Adapter Memory(*)\Dedicated Usage");
            }
            s.thermal = s.add(r"\Thermal Zone Information(*)\High Precision Temperature");
            s.energy = s.add(r"\Energy Meter(*)\Power");
            let _ = PdhCollectQueryData(s.query);
        }
        s
    }

    unsafe fn add(&self, path: &str) -> Option<PDH_HCOUNTER> {
        let w = wide(path);
        let mut c = PDH_HCOUNTER::default();
        (PdhAddEnglishCounterW(self.query, PCWSTR(w.as_ptr()), 0, &mut c) == 0).then_some(c)
    }

    pub fn sample(&mut self) -> Sample {
        let mut out = Sample { igpu: None, dgpu: None, dgpu_vram_mb: None, disk_mbs: 0.0, cpu_temp: None, cpu_watts: None };
        unsafe {
            if self.query.is_invalid() || PdhCollectQueryData(self.query) != 0 {
                return out;
            }
            if let Some(c) = self.disk {
                let mut v = PDH_FMT_COUNTERVALUE::default();
                if PdhGetFormattedCounterValue(c, PDH_FMT_DOUBLE, None, &mut v) == 0 && v.CStatus == 0 {
                    out.disk_mbs = (v.Anonymous.doubleValue / 1048576.0).max(0.0) as f32;
                }
            }
            if let Some(c) = self.engines {
                let util = engine_util(c);
                let get = |luid: &Option<String>| luid.as_ref().map(|l| util.get(l).copied().unwrap_or(0.0));
                out.igpu = get(&self.igpu_luid);
                out.dgpu = get(&self.dgpu_luid);
            }
            if let (Some(c), Some(l)) = (self.vram, &self.dgpu_luid) {
                out.dgpu_vram_mb = array(c)
                    .into_iter()
                    .filter(|(n, _)| n.contains(l.as_str()))
                    .map(|(_, v)| v)
                    .reduce(f64::max)
                    .map(|b| (b / 1048576.0) as f32);
            }
            if let Some(c) = self.energy {
                // RAPL_Package0_PKG (one per socket); per-core and _Total are not package power
                let mw: f64 = array(c).into_iter().filter(|(n, _)| n.to_ascii_lowercase().ends_with("_pkg")).map(|(_, v)| v).sum();
                out.cpu_watts = (mw > 0.0).then_some((mw / 1000.0) as f32);
            }
            if let Some(c) = self.thermal {
                out.cpu_temp = self.cpu_zone(array(c));
            }
        }
        out
    }

    /// The live CPU zone: one that has shown at least 3 distinct valid readings
    /// (fixed trip-point zones never do, even with a glitched first sample;
    /// empty zones read 0 K); among those the widest range wins.
    fn cpu_zone(&mut self, zones: Vec<(String, f64)>) -> Option<f32> {
        let mut best: Option<(i64, f32)> = None; // (range in tenths, current °C)
        for (name, tenth_k) in zones {
            let c = (tenth_k / 10.0 - 273.15) as f32;
            if !(1.0..=125.0).contains(&c) {
                continue;
            }
            let seen = self.zones.entry(name).or_default();
            let v = tenth_k.round() as i64;
            if !seen.contains(&v) && seen.len() < 64 {
                seen.push(v);
            }
            if seen.len() < 3 {
                continue;
            }
            let range = seen.iter().max().unwrap() - seen.iter().min().unwrap();
            if best.is_none_or(|(r, _)| range > r) {
                best = Some((range, c));
            }
        }
        best.map(|(_, c)| c)
    }
}

/// Adapter load the way Task Manager computes it: sum each engine across
/// processes, then take the busiest engine (summing all engines overshoots 100%).
unsafe fn engine_util(c: PDH_HCOUNTER) -> HashMap<String, f32> {
    let mut per_engine: HashMap<(String, String), f64> = HashMap::new();
    for (name, v) in array(c) {
        // pid_1234_luid_0x00000000_0x0000D1F2_phys_0_eng_3_engtype_3D
        let Some(li) = name.find("luid_") else { continue };
        let luid: String = name[li..].chars().take(26).collect();
        let eng = name.find("_eng_").map(|e| name[e..].to_string()).unwrap_or_default();
        *per_engine.entry((luid, eng)).or_default() += v;
    }
    let mut out: HashMap<String, f32> = HashMap::new();
    for ((luid, _), v) in per_engine {
        let e = out.entry(luid).or_default();
        *e = e.max(v.min(100.0) as f32);
    }
    out
}

unsafe fn array(c: PDH_HCOUNTER) -> Vec<(String, f64)> {
    let (mut size, mut count) = (0u32, 0u32);
    if PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE, &mut size, &mut count, None) != PDH_MORE_DATA || size == 0 {
        return Vec::new();
    }
    // u64 storage: the items hold f64/pointers and need 8-byte alignment.
    let mut buf = vec![0u64; size as usize / 8 + 2];
    let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
    if PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) != 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let it = &*items.add(i);
        if it.FmtValue.CStatus != 0 {
            continue;
        }
        out.push((pwstr(it.szName), it.FmtValue.Anonymous.doubleValue));
    }
    out
}

unsafe fn pwstr(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    p.to_string().unwrap_or_default()
}
