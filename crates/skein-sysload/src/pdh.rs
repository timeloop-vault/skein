//! Windows PDH query: English counter names, so it is locale-independent.

use std::sync::Mutex;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use windows::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW,
    PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows::core::PCWSTR;

use crate::Shared;
use crate::window::{self, LoadSample};

const ERROR_SUCCESS: u32 = 0;
// PDH_FMT_COUNTERVALUE.CStatus values that carry a usable reading.
const PDH_CSTATUS_VALID_DATA: u32 = 0;
const PDH_CSTATUS_NEW_DATA: u32 = 1;

struct Query(PDH_HQUERY);

impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful PdhOpenQueryW and is
        // closed exactly once, here.
        unsafe {
            PdhCloseQuery(self.0);
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn add(q: &Query, path: &str) -> Option<PDH_HCOUNTER> {
    let w = wide(path);
    let mut h = PDH_HCOUNTER::default();
    // SAFETY: `w` is a NUL-terminated UTF-16 string that outlives the call,
    // the query handle is open, and `h` is a valid out pointer.
    let rc = unsafe { PdhAddEnglishCounterW(q.0, PCWSTR(w.as_ptr()), 0, &raw mut h) };
    (rc == ERROR_SUCCESS).then_some(h)
}

fn read(h: PDH_HCOUNTER) -> Option<f64> {
    let mut v = PDH_FMT_COUNTERVALUE::default();
    // SAFETY: `h` belongs to a live query and `v` is a valid out pointer.
    let rc = unsafe { PdhGetFormattedCounterValue(h, PDH_FMT_DOUBLE, None, &raw mut v) };
    if rc != ERROR_SUCCESS || !matches!(v.CStatus, PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA) {
        return None;
    }
    // SAFETY: PDH_FMT_DOUBLE was requested and CStatus is success, so the
    // `doubleValue` union member is the initialised one.
    Some(unsafe { v.Anonymous.doubleValue })
}

fn collect(q: &Query) -> bool {
    // SAFETY: the query handle is open.
    unsafe { PdhCollectQueryData(q.0) == ERROR_SUCCESS }
}

pub(crate) fn run(shared: &Mutex<Shared>, interval: Duration, stop: &Receiver<()>) {
    let mut raw = PDH_HQUERY::default();
    // SAFETY: a null data source means real-time data; `raw` is a valid out pointer.
    if unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &raw mut raw) } != ERROR_SUCCESS {
        return;
    }
    let q = Query(raw);
    let drive = window::normalize_drive(std::env::var("SystemDrive").ok().as_deref());
    let (Some(cpu), Some(idle), Some(queue)) = (
        add(&q, window::CPU_PATH),
        add(&q, &window::idle_path(&drive)),
        add(&q, &window::queue_path(&drive)),
    ) else {
        return;
    };
    // Rate counters need a priming collection before the first value.
    if !collect(&q) {
        return;
    }
    loop {
        if !matches!(stop.recv_timeout(interval), Err(RecvTimeoutError::Timeout)) {
            return;
        }
        if !collect(&q) {
            continue;
        }
        let (Some(c), Some(i), Some(d)) = (read(cpu), read(idle), read(queue)) else {
            continue;
        };
        crate::record(
            shared,
            LoadSample {
                cpu_pct: window::clamp_pct(c),
                disk_busy_pct: window::busy_from_idle(i),
                disk_queue: window::clamp_queue(d),
            },
        );
    }
}
