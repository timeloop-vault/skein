//! A real child with an observer and no view at all (#732): the observer
//! hears `started` before any output, the child's output, and exactly one
//! exit.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::{PtyManager, PtyObserver, SpawnRequest};
use crate::harness_kind::HarnessKind;
use crate::spawn_settings::SpawnSettings;

#[derive(Default)]
struct Log {
    order: Vec<String>,
    text: String,
    exits: Vec<Option<u32>>,
}

#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Log>>);

impl PtyObserver for Seen {
    fn started(&self, injected: bool) {
        self.0.lock().order.push(format!("started:{injected}"));
    }
    fn output(&self, chunk: &str) {
        let mut st = self.0.lock();
        st.order.push("output".to_owned());
        st.text.push_str(chunk);
    }
    fn exited(&self, code: Option<u32>) {
        let mut st = self.0.lock();
        st.order.push("exited".to_owned());
        st.exits.push(code);
    }
}

fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[cfg(windows)]
const SHELL: &str = "cmd.exe";
#[cfg(not(windows))]
const SHELL: &str = "sh";

#[test]
fn an_observer_hears_a_child_with_no_view_attached() {
    let manager = PtyManager::new();
    let id = "observer-732";
    let cmd = vec![SHELL.to_owned()];
    let cwd = std::env::temp_dir();
    let settings = SpawnSettings::default();
    let seen = Seen::default();
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
            Some(Arc::new(seen.clone())),
        )
        .expect("spawn without a sink");
    // No view: the slot answers ConPTY's cursor query, so the shell starts.
    manager
        .write(id, b"echo skein-observed-732\r\n")
        .expect("write");
    assert!(
        wait_until(|| seen.0.lock().text.contains("skein-observed-732")),
        "the observer heard no output; got {:?}",
        seen.0.lock().text
    );

    manager.kill(id);
    assert!(
        wait_until(|| !seen.0.lock().exits.is_empty()),
        "the observer heard no exit"
    );
    // Let a duplicate show up if there were going to be one.
    std::thread::sleep(Duration::from_millis(300));
    let st = seen.0.lock();
    assert_eq!(st.exits.len(), 1, "exit is reported exactly once");
    assert_eq!(st.order.first().map(String::as_str), Some("started:false"));
    assert_eq!(st.order.last().map(String::as_str), Some("exited"));
}
