//! `PtyManager::kill` ends the child's whole process tree (#190), proved
//! through the real manager with a real shell and a real grandchild.
//!
//! The grandchild is the one whose command line carries `60` (the long
//! sleep/ping); an earlier short one carries `1` or `3`. It is identified by
//! pid plus start time, so a reused pid cannot make the check lie.

use std::path::Path;
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use super::procscan::{descendants, process_table};
use super::{PtyManager, SpawnRequest};
use crate::harness_kind::HarnessKind;
use crate::spawn_settings::SpawnSettings;

/// A process this test observed under its own PTY child.
#[derive(Clone, Copy, Debug)]
struct Seen {
    pid: u32,
    start: u64,
}

impl Seen {
    fn is_running(self) -> bool {
        let p = Pid::from_u32(self.pid);
        let mut sys = System::new();
        sys.refresh_processes(ProcessesToUpdate::Some(&[p]), true);
        sys.process(p).is_some_and(|q| q.start_time() == self.start)
    }

    /// Cleanup for a failed assertion: ends exactly this pid + start time,
    /// which the test saw as a descendant of its own PTY child.
    fn kill_if_running(self) {
        let p = Pid::from_u32(self.pid);
        let mut sys = System::new();
        sys.refresh_processes(ProcessesToUpdate::Some(&[p]), true);
        if let Some(q) = sys.process(p)
            && q.start_time() == self.start
        {
            q.kill();
        }
    }
}

/// Ends the recorded grandchild on drop, so a failed assertion leaks nothing.
struct Cleanup(Option<Seen>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(s) = self.0 {
            s.kill_if_running();
        }
    }
}

/// Descendants of `root` whose command line has an argument equal to `60`.
fn long_lived_descendant(root: u32) -> Option<Seen> {
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    let table = process_table(&sys);
    descendants(&table, root).into_iter().find_map(|pid| {
        let p = sys.process(Pid::from_u32(pid))?;
        p.cmd()
            .iter()
            .any(|a| a.to_string_lossy() == "60")
            .then(|| Seen {
                pid,
                start: p.start_time(),
            })
    })
}

fn poll(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    f()
}

fn spawn(manager: &PtyManager, id: &str, cmd: &[&str]) -> u32 {
    let cmd: Vec<String> = cmd.iter().map(|s| (*s).to_owned()).collect();
    let cwd = std::env::temp_dir();
    let settings = SpawnSettings::default();
    manager
        .spawn_with_sink(
            SpawnRequest {
                id: id.to_owned(),
                cmd: &cmd,
                cwd: Path::new(&cwd),
                rows: 24,
                cols: 80,
                settings: &settings,
                env_profile: None,
                agent: None,
                kind: HarnessKind::Byoh,
                harness_config: None,
                secret_env: Vec::new(),
                sidecar: None,
            },
            None,
            None,
            None,
        )
        .expect("spawn");
    manager.pid(id).expect("child pid")
}

fn find_grandchild(root: u32) -> Seen {
    let mut found = None;
    let ok = poll(Duration::from_secs(10), || {
        found = long_lived_descendant(root);
        found.is_some()
    });
    assert!(
        ok,
        "no long-lived descendant of pid {root} appeared within 10 s"
    );
    found.expect("checked above")
}

/// The grandchild must NOT share the PTY's console: dropping the pseudoconsole
/// ends every console-attached process by itself, which would hide a missing
/// tree kill. On Windows a hidden `Start-Process` gives `ping` its own console;
/// on unix `sh` ignores the terminal hangup in a subshell that then execs sleep.
#[cfg(windows)]
const SHELL_CMD: [&str; 5] = [
    "powershell.exe",
    "-NoProfile",
    "-NonInteractive",
    "-Command",
    "Start-Process ping -ArgumentList '-n','60','127.0.0.1' -WindowStyle Hidden; Start-Sleep -Seconds 60",
];
#[cfg(not(windows))]
const SHELL_CMD: [&str; 3] = ["sh", "-c", "(trap \"\" HUP; exec sleep 60) & wait"];

#[test]
fn kill_ends_the_grandchild() {
    let manager = PtyManager::new();
    let id = "tree-190-grandchild";
    let root = spawn(&manager, id, &SHELL_CMD);
    let mut cleanup = Cleanup(None);
    let grandchild = find_grandchild(root);
    cleanup.0 = Some(grandchild);
    assert!(grandchild.is_running());

    manager.kill(id);

    assert!(
        poll(Duration::from_secs(10), || !grandchild.is_running()),
        "grandchild {grandchild:?} of pid {root} still running 10 s after kill"
    );
}

/// The direct child exits on its own and a grandchild lives on; the job
/// object still holds it, so a later kill ends it.
#[cfg(windows)]
#[test]
fn kill_after_exit_ends_leftovers_on_windows() {
    let manager = PtyManager::new();
    let id = "tree-190-leftover";
    let root = spawn(
        &manager,
        id,
        &[
            "powershell.exe",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Process ping -ArgumentList '-n','60','127.0.0.1' -WindowStyle Hidden",
        ],
    );
    let mut cleanup = Cleanup(None);
    let leftover = find_grandchild(root);
    cleanup.0 = Some(leftover);

    assert!(
        poll(Duration::from_secs(10), || !manager.is_alive(id)),
        "the direct child did not exit on its own"
    );
    assert!(
        leftover.is_running(),
        "the grandchild {leftover:?} should outlive its exited parent"
    );

    manager.kill(id);

    assert!(
        poll(Duration::from_secs(10), || !leftover.is_running()),
        "leftover {leftover:?} still running 10 s after kill"
    );
}
