//! Boot sweep: kill sidecars a crashed Skein left running, and only those.
//! A process is selected by the pid we recorded, then confirmed by start
//! time and exe path; never by name or command line.

use std::path::Path;

use sysinfo::{Pid, ProcessesToUpdate, System};

use super::record::{self, Observed, PidRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SweepDecision {
    Kill,
    /// The pid exists but is some other process (pid reuse).
    LeaveMismatch,
    Gone,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepReport {
    pub killed: Vec<u32>,
    pub mismatched: Vec<u32>,
    pub gone: Vec<u32>,
}

fn exe_eq(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

pub(super) fn decide(rec: &PidRecord, observed: Option<&Observed>) -> SweepDecision {
    match observed {
        None => SweepDecision::Gone,
        Some(o) if o.start_time == rec.start_time && exe_eq(&o.exe, &rec.exe) => {
            SweepDecision::Kill
        }
        Some(_) => SweepDecision::LeaveMismatch,
    }
}

/// Read the record in `dir`, kill matching survivors, truncate the file.
pub fn sweep_orphans(dir: &Path) -> SweepReport {
    let mut report = SweepReport::default();
    if !record::record_path(dir).exists() {
        return report;
    }
    for rec in &record::read(dir) {
        let observed = record::observe(rec.pid);
        match decide(rec, observed.as_ref()) {
            SweepDecision::Gone => {
                tracing::debug!(harness = %rec.harness_id, pid = rec.pid, "sidecar sweep: already gone");
                report.gone.push(rec.pid);
            }
            SweepDecision::LeaveMismatch => {
                tracing::warn!(
                    harness = %rec.harness_id, pid = rec.pid,
                    "sidecar sweep: pid now belongs to a different process, leaving it"
                );
                report.mismatched.push(rec.pid);
            }
            SweepDecision::Kill => {
                // Descendants (its stdio MCP servers) are taken from the
                // verified root only, before it dies and orphans them;
                // `kill_all` revalidates each by start time.
                let descendants = super::tree::collect(rec.pid);
                let killed = kill_pid(rec.pid);
                super::tree::kill_all(&descendants);
                if killed {
                    tracing::info!(harness = %rec.harness_id, pid = rec.pid, "sidecar sweep: killed orphan");
                    report.killed.push(rec.pid);
                } else {
                    tracing::warn!(harness = %rec.harness_id, pid = rec.pid, "sidecar sweep: kill failed");
                    report.mismatched.push(rec.pid);
                }
            }
        }
    }
    if let Err(e) = record::write(dir, &[]) {
        tracing::warn!(error = %e, "sidecar sweep: truncating record failed");
    }
    report
}

fn kill_pid(pid: u32) -> bool {
    let mut sys = System::new();
    let p = Pid::from_u32(pid);
    sys.refresh_processes(ProcessesToUpdate::Some(&[p]), true);
    sys.process(p).is_some_and(sysinfo::Process::kill)
}
