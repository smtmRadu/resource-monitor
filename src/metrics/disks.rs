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

/// Fallback DISK snake full scale when WinSAT has no disk score.
const DEFAULT_MAX_MBS: f32 = 512.0;

/// System disk's max transfer speed (MB/s): the sequential read throughput of
/// Windows' own WinSAT assessment (run at setup; small XML files, read once at
/// start). 512 MB/s when no assessment exists.
pub fn max_mbs() -> f32 {
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into()) + r"\Performance\WinSAT\DataStore";
    let Ok(rd) = std::fs::read_dir(dir) else { return DEFAULT_MAX_MBS };
    // File names start with a timestamp: newest last.
    let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().ends_with(".WinSAT.xml")).collect();
    files.sort();
    files.iter().rev().find_map(|p| seq_read(&std::fs::read(p).ok()?)).unwrap_or(DEFAULT_MAX_MBS)
}

/// `<AvgThroughput kind="Sequential Read" ...>6416.27</AvgThroughput>` from a UTF-16LE WinSAT XML.
fn seq_read(bytes: &[u8]) -> Option<f32> {
    let utf16: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let xml = String::from_utf16_lossy(&utf16);
    let rest = &xml[xml.find("<AvgThroughput kind=\"Sequential Read\"")?..];
    let start = rest.find('>')? + 1;
    let end = start + rest[start..].find('<')?;
    rest[start..end].trim().parse::<f32>().ok().filter(|&v| v > 0.0)
}
