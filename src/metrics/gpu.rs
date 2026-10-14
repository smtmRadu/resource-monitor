//! Adapter enumeration (D3DKMT), vendor detection and display refresh rate.
//!
//! D3DKMT kernel-thunk queries instead of DXGI: creating a DXGI factory loads
//! every adapter's user-mode driver and woke the sleeping NVIDIA dGPU on a
//! hybrid laptop (P8 2 W -> P0 17 W). These queries leave it asleep.
use std::ffi::c_void;
use std::sync::OnceLock;

use windows::core::w;
use windows::Win32::Graphics::Gdi::{EnumDisplaySettingsW, DEVMODEW, ENUM_CURRENT_SETTINGS};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Vendor {
    Intel,
    Amd,
    Nvidia,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct Adapter {
    pub name: String,
    /// "luid_0x{high:08X}_0x{low:08X}" as it appears in PDH GPU instance names.
    pub luid: String,
    pub vendor: Vendor,
    pub dedicated_mb: f32,
    pub is_dgpu: bool,
    pub is_igpu: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct KmtAdapterInfo {
    h_adapter: u32,
    luid_low: u32,
    luid_high: i32,
    num_sources: u32,
    precise: i32,
}

#[repr(C)]
struct KmtEnumAdapters2 {
    num: u32,
    adapters: *mut KmtAdapterInfo,
}

#[repr(C)]
struct KmtQuery {
    h_adapter: u32,
    kind: u32,
    data: *mut c_void,
    size: u32,
}

#[link(name = "gdi32")]
extern "system" {
    fn D3DKMTEnumAdapters2(p: *mut KmtEnumAdapters2) -> i32;
    fn D3DKMTQueryAdapterInfo(p: *const KmtQuery) -> i32;
    fn D3DKMTCloseAdapter(p: *const u32) -> i32;
}

const QAI_SEGMENTSIZE: u32 = 3;
const QAI_REGISTRYINFO: u32 = 8;
const QAI_ADAPTERTYPE: u32 = 15;
const QAI_DEVICEIDS: u32 = 31;
const TYPE_SOFTWARE: u32 = 1 << 2;
const TYPE_HYBRID_DISCRETE: u32 = 1 << 4;
const TYPE_HYBRID_INTEGRATED: u32 = 1 << 5;
const TYPE_INDIRECT: u32 = 1 << 6;

unsafe fn query<T>(h: u32, kind: u32, out: &mut T) -> bool {
    let q = KmtQuery { h_adapter: h, kind, data: out as *mut T as *mut c_void, size: std::mem::size_of::<T>() as u32 };
    D3DKMTQueryAdapterInfo(&q) >= 0
}

fn vendor_of(id: u32) -> Vendor {
    match id {
        0x10DE => Vendor::Nvidia,
        0x1002 | 0x1022 => Vendor::Amd,
        0x8086 => Vendor::Intel,
        _ => Vendor::Unknown,
    }
}

fn enumerate() -> Vec<Adapter> {
    let mut out: Vec<(Adapter, u32)> = Vec::new();
    unsafe {
        let mut e = KmtEnumAdapters2 { num: 0, adapters: std::ptr::null_mut() };
        if D3DKMTEnumAdapters2(&mut e) < 0 || e.num == 0 {
            return Vec::new();
        }
        let mut list = vec![KmtAdapterInfo::default(); e.num as usize];
        e.adapters = list.as_mut_ptr();
        if D3DKMTEnumAdapters2(&mut e) < 0 {
            return Vec::new();
        }
        for a in &list[..e.num as usize] {
            let mut kind = 0u32;
            query(a.h_adapter, QAI_ADAPTERTYPE, &mut kind);
            let mut ids = [0u32; 7]; // PhysicalAdapterIndex + VendorID, DeviceID, ...
            query(a.h_adapter, QAI_DEVICEIDS, &mut ids);
            let mut seg = [0u64; 3]; // dedicated video, dedicated system, shared system
            query(a.h_adapter, QAI_SEGMENTSIZE, &mut seg);
            let mut reg = [0u16; 260 * 4]; // AdapterString first
            query(a.h_adapter, QAI_REGISTRYINFO, &mut reg);
            let _ = D3DKMTCloseAdapter(&a.h_adapter);
            if kind & (TYPE_SOFTWARE | TYPE_INDIRECT) != 0 || ids[1] == 0x1414 {
                continue; // Basic Render Driver, virtual displays
            }
            let luid = format!("luid_0x{:08X}_0x{:08X}", a.luid_high as u32, a.luid_low);
            if out.iter().any(|(x, _)| x.luid == luid) {
                continue;
            }
            let n = reg[..260].iter().position(|&c| c == 0).unwrap_or(260);
            out.push((
                Adapter {
                    name: String::from_utf16_lossy(&reg[..n]),
                    luid,
                    vendor: vendor_of(ids[1]),
                    dedicated_mb: seg[0] as f32 / 1048576.0,
                    is_dgpu: false,
                    is_igpu: false,
                },
                kind,
            ));
        }
    }
    let mut v: Vec<Adapter> = out.iter().map(|(a, _)| a.clone()).collect();
    if v.len() >= 2 {
        // Prefer the driver's own hybrid flags; fall back to dedicated VRAM size.
        let by_mem_lo = (0..v.len()).min_by(|&a, &b| v[a].dedicated_mb.total_cmp(&v[b].dedicated_mb)).unwrap();
        let by_mem_hi = (0..v.len()).max_by(|&a, &b| v[a].dedicated_mb.total_cmp(&v[b].dedicated_mb)).unwrap();
        let ig = out.iter().position(|(_, k)| k & TYPE_HYBRID_INTEGRATED != 0).unwrap_or(by_mem_lo);
        let dg = out.iter().position(|(_, k)| k & TYPE_HYBRID_DISCRETE != 0).unwrap_or(by_mem_hi);
        v[ig].is_igpu = true;
        if dg != ig {
            v[dg].is_dgpu = true;
        }
    } else if let Some(a) = v.first_mut() {
        a.is_dgpu = true;
    }
    v
}

/// Hardware adapters, enumerated once per process.
pub fn adapters() -> Vec<Adapter> {
    static CACHE: OnceLock<Vec<Adapter>> = OnceLock::new();
    CACHE.get_or_init(enumerate).clone()
}

pub fn cpu_vendor() -> Vendor {
    let id = std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_default().to_lowercase();
    if id.contains("authenticamd") {
        Vendor::Amd
    } else if id.contains("genuineintel") {
        Vendor::Intel
    } else {
        Vendor::Unknown
    }
}

/// CPU model, short ("Core Ultra 9 275HX"); empty when unknown.
pub fn cpu_name() -> String {
    short_name(&cpu_full_name())
}

fn cpu_full_name() -> String {
    let mut buf = [0u16; 128];
    let mut len = (buf.len() * 2) as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"),
            w!("ProcessorNameString"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    }
    .is_ok();
    let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
    if ok { String::from_utf16_lossy(&buf[..n]) } else { String::new() }
}

/// iGPU model, short ("Radeon 780M Graphics"): the adapter's name, else the
/// CPU name's " w/ ..." / " with ..." suffix; empty when neither exists.
pub fn igpu_name() -> String {
    if let Some(a) = adapters().iter().find(|a| a.is_igpu) {
        return short_name(&a.name);
    }
    let cpu = cpu_full_name();
    [" w/ ", " with "].iter().find_map(|sep| cpu.split_once(sep)).map(|(_, g)| short_name(g)).unwrap_or_default()
}

/// dGPU model, short ("GeForce RTX 5070 Ti Laptop"); empty without a dGPU.
pub fn dgpu_name() -> String {
    adapters().iter().find(|a| a.is_dgpu).map(|a| short_name(&a.name)).unwrap_or_default()
}

/// Drop vendor names, trademarks and filler from a marketing name.
fn short_name(s: &str) -> String {
    let s = s.split(" @ ").next().unwrap_or(s); // "... CPU @ 2.40GHz"
    let s = s.split(" with ").next().unwrap_or(s); // "... with Radeon Graphics"
    let s = s.split(" w/ ").next().unwrap_or(s); // "... w/ Radeon 780M Graphics"
    const DROP: &[&str] = &["(R)", "(TM)", "(tm)", "NVIDIA", "AMD", "Intel", "CPU", "GPU", "Processor"];
    s.split_whitespace()
        .map(|w| DROP.iter().fold(w.to_string(), |w, d| w.replace(d, "")))
        .filter(|w| !w.is_empty() && !w.ends_with("-Core"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn dgpu_vendor() -> Vendor {
    adapters().iter().find(|a| a.is_dgpu).map(|a| a.vendor).unwrap_or(Vendor::Unknown)
}

/// Primary display refresh rate (FPS normalization), fallback 144.
pub fn display_hz() -> u32 {
    let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
    let ok = unsafe { EnumDisplaySettingsW(None, ENUM_CURRENT_SETTINGS, &mut dm) }.as_bool();
    if ok && dm.dmDisplayFrequency > 1 {
        dm.dmDisplayFrequency
    } else {
        144
    }
}
