//! A real child with no sink attached at spawn (#731).
//!
//! `ConPTY` withholds all child output until the host answers its `ESC[6n`
//! cursor query (#484). With no view attached, the daemon's `SinkSlot`
//! answers; this proves it end to end. Without that answer, the child's
//! output never arrives and `echo` below would time out — not asserted as
//! a negative test, because "nothing arrived" is only provable by waiting
//! and would be slow and timing-dependent.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use sysinfo::{Pid, ProcessesToUpdate, System};

use super::{PtyEvent, PtyManager, SpawnRequest};
use crate::harness_kind::HarnessKind;
use crate::spawn_settings::SpawnSettings;
use crate::stream_sink::StreamSink;

#[derive(Clone, Default)]
struct Recording(Arc<Mutex<String>>);

impl Recording {
    fn text(&self) -> String {
        self.0.lock().clone()
    }
}

impl StreamSink<PtyEvent> for Recording {
    fn send(&self, item: PtyEvent) -> Result<(), String> {
        if let PtyEvent::Data { chunk } = item {
            self.0.lock().push_str(&chunk);
        }
        Ok(())
    }
}

fn wait_for(rec: &Recording, needle: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if rec.text().contains(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn pid_exists(pid: u32) -> bool {
    let p = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[p]), true);
    sys.process(p).is_some()
}

#[cfg(windows)]
const SHELL: &str = "cmd.exe";
#[cfg(not(windows))]
const SHELL: &str = "sh";

#[test]
fn a_pty_spawned_without_a_sink_works_once_a_view_attaches() {
    let manager = PtyManager::new();
    let id = "sink-731";
    let cmd = vec![SHELL.to_owned()];
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
        )
        .expect("spawn without a sink");
    let pid = manager.pid(id).expect("child pid");
    // Let the start-up output (and ConPTY's cursor query) go unattended.
    std::thread::sleep(Duration::from_millis(500));

    let first = Recording::default();
    manager.attach(id, Box::new(first.clone())).expect("attach");
    manager
        .write(id, b"echo skein-marker-731\r\n")
        .expect("write");
    assert!(
        wait_for(&first, "skein-marker-731"),
        "no output after attach; got {:?}",
        first.text()
    );

    assert!(manager.detach(id), "a sink was attached");
    assert!(manager.is_alive(id));
    assert!(pid_exists(pid), "the child survives a detach");

    let second = Recording::default();
    manager
        .attach(id, Box::new(second.clone()))
        .expect("re-attach");
    manager
        .write(id, b"echo skein-second-731\r\n")
        .expect("write");
    assert!(
        wait_for(&second, "skein-second-731"),
        "no output after re-attach; got {:?}",
        second.text()
    );
    assert!(
        !second.text().contains("skein-marker-731"),
        "the new sink must not replay the old sink's output"
    );

    manager.kill(id);
    assert!(!manager.is_alive(id));
}
