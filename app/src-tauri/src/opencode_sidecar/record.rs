//! The PID record: which sidecars this Skein started, so the next boot can
//! find the ones a crash left behind. No secrets in it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

pub(super) const RECORD_FILE: &str = "opencode-sidecars.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PidRecord {
    pub harness_id: String,
    pub pid: u32,
    /// Seconds since the epoch, as sysinfo reports it.
    pub start_time: u64,
    /// Executable path as sysinfo reports it.
    pub exe: String,
}

/// What the OS says a pid is right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Observed {
    pub start_time: u64,
    pub exe: String,
}

pub(super) fn record_path(dir: &Path) -> PathBuf {
    dir.join(RECORD_FILE)
}

/// Read the file; a missing or corrupt file reads as empty.
pub(super) fn read(dir: &Path) -> Vec<PidRecord> {
    let Ok(text) = std::fs::read_to_string(record_path(dir)) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "opencode sidecar record unreadable, treating as empty");
        Vec::new()
    })
}

/// Atomic write: temp file, then rename over the record.
pub(super) fn write(dir: &Path, records: &[PidRecord]) -> std::io::Result<()> {
    let path = record_path(dir);
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(records).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)
}

/// Replace any entry for the harness with `rec`. Callers serialise.
pub(super) fn add(dir: &Path, rec: PidRecord) {
    let mut records = read(dir);
    records.retain(|r| r.harness_id != rec.harness_id);
    records.push(rec);
    if let Err(e) = write(dir, &records) {
        tracing::warn!(error = %e, "opencode sidecar record write failed");
    }
}

/// Drop the entry for `harness_id` if it still names `pid`.
pub(super) fn remove(dir: &Path, harness_id: &str, pid: u32) {
    let mut records = read(dir);
    let before = records.len();
    records.retain(|r| !(r.harness_id == harness_id && r.pid == pid));
    if records.len() != before
        && let Err(e) = write(dir, &records)
    {
        tracing::warn!(error = %e, "opencode sidecar record write failed");
    }
}

fn refresh_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing().with_exe(UpdateKind::Always)
}

/// Look up one pid. `None` when it does not exist or its exe is hidden.
pub(super) fn observe(pid: u32) -> Option<Observed> {
    let mut sys = System::new();
    let p = Pid::from_u32(pid);
    sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[p]), true, refresh_kind());
    let proc = sys.process(p)?;
    Some(Observed {
        start_time: proc.start_time(),
        exe: proc.exe()?.to_string_lossy().into_owned(),
    })
}

/// Build a record for a just-spawned pid, if the OS will describe it.
pub(super) fn describe(harness_id: &str, pid: u32) -> Option<PidRecord> {
    let o = observe(pid)?;
    Some(PidRecord {
        harness_id: harness_id.to_owned(),
        pid,
        start_time: o.start_time,
        exe: o.exe,
    })
}
