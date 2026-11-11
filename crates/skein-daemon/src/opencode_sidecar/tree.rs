//! Descendants of a sidecar, so a kill can take its stdio MCP servers
//! with it. Only processes found under OUR child's pid in the live table
//! are ever touched; nothing is selected by name.

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use crate::pty::procscan::{descendants, process_table};

fn snapshot() -> System {
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    sys
}

/// `(pid, start_time)` of every transitive child of `root`. The start time
/// guards `kill_all` against a pid reused in between.
pub(super) fn collect(root: u32) -> Vec<(u32, u64)> {
    let sys = snapshot();
    descendants(&process_table(&sys), root)
        .into_iter()
        .filter_map(|pid| {
            let p = sys.process(Pid::from_u32(pid))?;
            Some((pid, p.start_time()))
        })
        .collect()
}

/// Kill the collected processes that are still the same process.
pub(super) fn kill_all(found: &[(u32, u64)]) {
    if found.is_empty() {
        return;
    }
    let sys = snapshot();
    for (pid, start) in found {
        if let Some(p) = sys.process(Pid::from_u32(*pid))
            && p.start_time() == *start
        {
            p.kill();
        }
    }
}
