//! Pane notices through a real PTY (#809, T-5): the observer's exit notice and
//! a queued sidecar notice reach the viewer in frames mode, in a frame before
//! `Exit`, and never in raw mode.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::frames::TerminalMode;
use super::{PtyEvent, PtyManager, PtyObserver, SpawnRequest};
use crate::harness_kind::HarnessKind;
use crate::spawn_settings::SpawnSettings;
use crate::stream_sink::StreamSink;

#[cfg(windows)]
const SHELL: &str = "cmd.exe";
#[cfg(not(windows))]
const SHELL: &str = "sh";

#[derive(Clone, Default)]
struct Rec(Arc<Mutex<Vec<String>>>);

impl Rec {
    /// `data:<chunk>` and `exit` entries, in order.
    fn events(&self) -> Vec<String> {
        self.0.lock().clone()
    }
    fn exited(&self) -> bool {
        self.events().iter().any(|e| e == "exit")
    }
    fn index_of(&self, needle: &str) -> Option<usize> {
        self.events().iter().position(|e| e.contains(needle))
    }
}

impl StreamSink<PtyEvent> for Rec {
    fn send(&self, item: PtyEvent) -> Result<(), String> {
        match item {
            PtyEvent::Data { chunk } => self.0.lock().push(format!("data:{chunk}")),
            PtyEvent::Exit { .. } => self.0.lock().push("exit".to_owned()),
            PtyEvent::Grid { .. } => {}
        }
        Ok(())
    }
}

struct Banner;

impl PtyObserver for Banner {
    fn started(&self, _: bool) {}
    fn output(&self, _: &str) {}
    fn exited(&self, _: Option<u32>) {}
    fn exit_notice(&self, code: Option<u32>) -> Option<Vec<u8>> {
        Some(format!("\r\nEXIT-NOTICE-{}\r\n", code.unwrap_or(999)).into_bytes())
    }
}

fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("timed out waiting for {what}");
}

fn spawn(manager: &PtyManager, rec: &Rec) {
    let cmd = vec![SHELL.to_owned()];
    let cwd = std::env::temp_dir();
    let settings = SpawnSettings::default();
    manager
        .spawn_with_sink(
            SpawnRequest {
                id: "n-1".to_owned(),
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
                sidecar: Some(("h-1".to_owned(), 7)),
            },
            Some(Box::new(rec.clone())),
            None,
            Some(Arc::new(Banner)),
        )
        .expect("spawn");
}

fn quit(manager: &PtyManager) {
    manager.write("n-1", b"exit\r\n").expect("write");
}

#[test]
fn frames_mode_appends_the_exit_notice_then_the_queued_one_before_exit() {
    let manager = PtyManager::with_terminal_mode(TerminalMode::Frames);
    let rec = Rec::default();
    spawn(&manager, &rec);
    let taken = manager.append_notice_bound("h-1", 7, b"\r\nQUEUED-NOTICE\r\n");
    assert_eq!(taken, 1);
    assert_eq!(
        manager.append_notice_bound("h-1", 8, b"x"),
        0,
        "other generation"
    );
    wait_until("the shell to be up", || !rec.events().is_empty());
    assert!(rec.index_of("QUEUED-NOTICE").is_none(), "queued while live");

    quit(&manager);
    wait_until("exit", || rec.exited());
    let (exit_notice, queued, exit) = (
        rec.index_of("EXIT-NOTICE-").expect("exit notice"),
        rec.index_of("QUEUED-NOTICE").expect("queued notice"),
        rec.events().len() - 1,
    );
    assert!(exit_notice <= queued && queued < exit, "{:?}", rec.events());
    let screen = manager.screen_text("n-1").expect("engine").join("\n");
    assert!(screen.contains("QUEUED-NOTICE"), "{screen}");
}

#[test]
fn raw_mode_writes_no_notice() {
    let manager = PtyManager::new();
    let rec = Rec::default();
    spawn(&manager, &rec);
    assert_eq!(manager.append_notice_bound("h-1", 7, b"QUEUED-NOTICE"), 0);
    // No view answers `ConPTY`'s cursor query here (raw mode forwards it).
    #[cfg(windows)]
    manager.write("n-1", b"[1;1R").expect("write");
    quit(&manager);
    wait_until("exit", || rec.exited());
    assert!(rec.index_of("EXIT-NOTICE-").is_none());
    assert!(rec.index_of("QUEUED-NOTICE").is_none());
}
