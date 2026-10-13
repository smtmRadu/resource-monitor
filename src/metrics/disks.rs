//! Fixed drives: used fraction per drive letter + which physical disk it lives on.
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES,
};
use windows::Win32::System::IO::DeviceIoControl;

const DRIVE_FIXED: u32 = 3;
const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;

#[derive(Clone, Debug)]
pub struct Drive {
    pub letter: char,
    pub used: f32, // 0..1
    pub total_bytes: u64,
    /// Physical disk number (PhysicalDriveN); u32::MAX when unknown.
    pub disk: u32,
}

pub struct Disks {
    drives: Vec<Drive>,
    usage_at: Option<Instant>,
    map_at: Option<Instant>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn fixed_letters() -> Vec<char> {
    let mut buf = [0u16; 512];
    let n = unsafe { GetLogicalDriveStringsW(Some(&mut buf)) } as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
        .split('\0')
        .filter(|r| !r.is_empty())
        .filter(|r| unsafe { GetDriveTypeW(PCWSTR(wide(r).as_ptr())) } == DRIVE_FIXED)
        .filter_map(|r| r.chars().next())
        .collect()
}

/// (used fraction, total bytes)
fn usage(letter: char) -> Option<(f32, u64)> {
    let root = wide(&format!("{letter}:\\"));
    let (mut free, mut total) = (0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(PCWSTR(root.as_ptr()), None, Some(&mut total), Some(&mut free)) }.ok()?;
    (total > 0).then(|| ((1.0 - free as f64 / total as f64) as f32, total))
}

/// Physical disk of the volume's first extent (no admin needed: zero access rights).
fn physical_disk(letter: char) -> u32 {
    unsafe {
        let path = wide(&format!("\\\\.\\{letter}:"));
        let Ok(h) = CreateFileW(
            PCWSTR(path.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        ) else {
            return u32::MAX;
        };
        // VOLUME_DISK_EXTENTS: NumberOfDiskExtents (+pad), then {DiskNumber (+pad), offset, length}...
        let mut out = [0u64; 32];
        let mut got = 0u32;
        let ok = DeviceIoControl(
            h,
            IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
            None,
            0,
            Some(out.as_mut_ptr() as *mut _),
            std::mem::size_of_val(&out) as u32,
            Some(&mut got),
            None,
        )
        .is_ok();
        let _ = CloseHandle(h);
        if ok && got >= 16 && out[0] as u32 >= 1 {
            out[1] as u32
        } else {
            u32::MAX
        }
    }
}

impl Disks {
    pub fn new() -> Self {
        Self { drives: Vec::new(), usage_at: None, map_at: None }
    }

    /// Drive letters/physical mapping refresh every 60s, usage every 5s
    /// (GetDiskFreeSpaceExW reads filesystem metadata: microseconds, no scan).
    pub fn sample(&mut self) -> Vec<Drive> {
        if self.map_at.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
            self.drives = fixed_letters()
                .into_iter()
                .map(|letter| Drive { letter, used: 0.0, total_bytes: 0, disk: physical_disk(letter) })
                .collect();
            self.map_at = Some(Instant::now());
            self.usage_at = None;
        }
        if self.usage_at.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
            self.drives.retain_mut(|d| match usage(d.letter) {
                Some((u, total)) => {
                    d.used = u;
                    d.total_bytes = total;
                    true
                }
                None => false,
            });
            self.usage_at = Some(Instant::now());
        }
        self.drives.clone()
    }
}
