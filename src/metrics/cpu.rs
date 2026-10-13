//! CPU load (total + per logical core) and RAM.
use std::ffi::c_void;

use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

const SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION: u32 = 8;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Sppi {
    idle: i64,
    kernel: i64, // includes idle
    user: i64,
    dpc: i64,
    interrupt: i64,
    interrupt_count: u32,
}

#[link(name = "ntdll")]
extern "system" {
    fn NtQuerySystemInformation(class: u32, buf: *mut c_void, len: u32, ret: *mut u32) -> i32;
}

/// (busy+idle, idle) time per logical processor in the current group.
fn read() -> Vec<(i64, i64)> {
    let mut buf = vec![Sppi::default(); 256];
    let mut ret = 0u32;
    let st = unsafe {
        NtQuerySystemInformation(
            SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION,
            buf.as_mut_ptr() as *mut c_void,
            (buf.len() * std::mem::size_of::<Sppi>()) as u32,
            &mut ret,
        )
    };
    if st < 0 {
        return Vec::new();
    }
    buf.truncate(ret as usize / std::mem::size_of::<Sppi>());
    buf.iter().map(|s| (s.kernel + s.user, s.idle)).collect()
}

pub fn core_count() -> usize {
    read().len()
}

pub struct Cpu {
    prev: Vec<(i64, i64)>,
}

impl Cpu {
    pub fn new() -> Self {
        Self { prev: read() }
    }

    /// Busy % since the previous call: (total, per-core).
    pub fn sample(&mut self) -> (f32, Vec<f32>) {
        let cur = read();
        let mut cores = Vec::with_capacity(cur.len());
        let (mut all, mut idle) = (0i64, 0i64);
        for (i, &(t, id)) in cur.iter().enumerate() {
            let (pt, pi) = self.prev.get(i).copied().unwrap_or((t, id));
            let (dt, di) = ((t - pt).max(0), (id - pi).max(0));
            all += dt;
            idle += di;
            cores.push(busy(dt, di));
        }
        self.prev = cur;
        (busy(all, idle), cores)
    }
}

fn busy(total: i64, idle: i64) -> f32 {
    if total <= 0 {
        return 0.0;
    }
    ((total - idle.min(total)) as f64 / total as f64 * 100.0) as f32
}

/// (percent, used GB, total GB) — used = total - available, like psutil.
pub fn ram() -> (f32, f32, f32) {
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    if unsafe { GlobalMemoryStatusEx(&mut m) }.is_err() || m.ullTotalPhys == 0 {
        return (0.0, 0.0, 0.0);
    }
    let gb = 1024.0f64.powi(3);
    let used = m.ullTotalPhys.saturating_sub(m.ullAvailPhys) as f64;
    let total = m.ullTotalPhys as f64;
    ((used / total * 100.0) as f32, (used / gb) as f32, (total / gb) as f32)
}
