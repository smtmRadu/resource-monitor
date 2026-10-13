//! Child-process lifetime + elevation helpers.
//!
//! Helpers (PresentMon, the LHM bridge) are put in a kill-on-close job, so they
//! die with the monitor even if it is killed hard. The Python version left
//! orphaned bridge processes behind.
use std::os::windows::io::AsRawHandle;
use std::sync::OnceLock;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::*;
use windows::Win32::UI::Shell::IsUserAnAdmin;

struct Job(HANDLE);
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

fn job() -> Option<HANDLE> {
    static JOB: OnceLock<Option<Job>> = OnceLock::new();
    JOB.get_or_init(|| unsafe {
        let h = CreateJobObjectW(None, None).ok()?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            h,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .ok()?;
        Some(Job(h))
    })
    .as_ref()
    .map(|j| j.0)
}

pub fn adopt(child: &std::process::Child) {
    if let Some(j) = job() {
        unsafe {
            let _ = AssignProcessToJobObject(j, HANDLE(child.as_raw_handle()));
        }
    }
}

pub fn is_elevated() -> bool {
    unsafe { IsUserAnAdmin().as_bool() }
}

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
