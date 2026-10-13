//! Network throughput over physical adapters only.
//!
//! psutil summed every interface, so WSL / Hyper-V vEthernet traffic was
//! counted twice (once on the virtual switch, once on the real NIC).
//! GetIfTable2 walks every interface including filter layers (~4 ms), so the
//! physical NICs are picked once every 30s and read with GetIfEntry2 per tick.
//! Deltas are per adapter, so a NIC that joins (Wi-Fi reconnect, cable plugged
//! in) contributes from its second reading instead of its lifetime counters.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfEntry2, GetIfTable2, MIB_IF_ROW2, MIB_IF_TABLE2};
use windows::Win32::NetworkManagement::Ndis::{IfOperStatusUp, NET_LUID_LH};

pub struct Net {
    nics: Vec<NET_LUID_LH>,
    picked: Option<Instant>,
    /// LUID -> (rx, tx) at `at`
    prev: HashMap<u64, (u64, u64)>,
    at: Instant,
}

fn physical_nics() -> Vec<NET_LUID_LH> {
    let mut out = Vec::new();
    unsafe {
        let mut t: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut t).is_err() || t.is_null() {
            return out;
        }
        let rows = std::slice::from_raw_parts((*t).Table.as_ptr(), (*t).NumEntries as usize);
        for r in rows {
            let flags = r.InterfaceAndOperStatusFlags._bitfield;
            let (hardware, filter) = (flags & 0x1 != 0, flags & 0x2 != 0);
            if hardware && !filter && r.OperStatus == IfOperStatusUp {
                out.push(r.InterfaceLuid);
            }
        }
        FreeMibTable(t as *const _);
    }
    out
}

impl Net {
    pub fn new() -> Self {
        let mut n = Self { nics: Vec::new(), picked: None, prev: HashMap::new(), at: Instant::now() };
        n.prev = n.counters();
        n
    }

    fn counters(&mut self) -> HashMap<u64, (u64, u64)> {
        if self.picked.is_none_or(|t| t.elapsed() > Duration::from_secs(30)) {
            self.nics = physical_nics();
            self.picked = Some(Instant::now());
        }
        let mut out = HashMap::new();
        for luid in &self.nics {
            let mut row = MIB_IF_ROW2 { InterfaceLuid: *luid, ..Default::default() };
            if unsafe { GetIfEntry2(&mut row) }.is_ok() {
                out.insert(unsafe { luid.Value }, (row.InOctets, row.OutOctets));
            }
        }
        out
    }

    /// (down, up) in MB/s since the previous call.
    pub fn sample(&mut self) -> (f32, f32) {
        let cur = self.counters();
        let now = Instant::now();
        let dt = now.duration_since(self.at).as_secs_f64().max(0.2);
        let (mut rx, mut tx) = (0u64, 0u64);
        for (luid, &(r, t)) in &cur {
            if let Some(&(pr, pt)) = self.prev.get(luid) {
                rx += r.saturating_sub(pr);
                tx += t.saturating_sub(pt);
            }
        }
        self.prev = cur;
        self.at = now;
        let mb = 1048576.0;
        ((rx as f64 / dt / mb) as f32, (tx as f64 / dt / mb) as f32)
    }
}
