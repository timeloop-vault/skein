//! Tests for the harness runner (#731). The fixture builds a whole daemon
//! with no client and no Tauri; the Claude transcript root is redirected to
//! a temp dir, so nothing here reads or writes the real home directory.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{Value, json};
use skein_proto::room_mutations::RoomMutation;
use skein_proto::rooms::{Harness, Room};
use sysinfo::{Pid, ProcessesToUpdate, System};
use tempfile::TempDir;

use super::{EnsureError, EnsureOutcome, EnsureRequest, HarnessRunner};
use crate::agent_api::client::{ClientRequests, Rect, Shot, WebviewCapture};
use crate::agent_api::state::AgentRequest;
use crate::daemon::{Daemon, Probe, spawn_env};
use crate::event_sink::EventSink;
use crate::paths::Paths;
use crate::pty::PtyEvent;
use crate::stream_sink::StreamSink;
use crate::transport::InProcessTransport;

mod acceptance;
mod client_input;
mod derived;
mod derived_boot;
mod hermes_work;
mod lifecycle;
mod phase;
mod record_race;
mod refusals;
mod stops;

struct NoEvents;

impl EventSink for NoEvents {
    fn emit_json(&self, _event: &str, _payload: String) -> Result<(), String> {
        Ok(())
    }
}

struct NoClient;

impl ClientRequests for NoClient {
    fn send(&self, _request: &AgentRequest) -> Result<(), String> {
        Err("no client".to_owned())
    }
}

impl WebviewCapture for NoClient {
    fn capture(
        &self,
        _rect: Rect,
        _scale: f64,
        _done: Box<dyn FnOnce(Result<Shot, String>) + Send>,
    ) -> Result<(), String> {
        Err("no client".to_owned())
    }
}

/// A long-lived stand-in for the CLI: an interactive shell.
#[cfg(windows)]
const SHELL: &str = "cmd.exe";
#[cfg(not(windows))]
const SHELL: &str = "sh";

fn shell_cmd() -> Vec<String> {
    vec![SHELL.to_owned()]
}

/// A stand-in that exits by itself with code 3.
fn exit_3_cmd() -> Vec<String> {
    let flag = if cfg!(windows) { "/c" } else { "-c" };
    vec![SHELL.to_owned(), flag.to_owned(), "exit 3".to_owned()]
}

/// Records what a view would see.
#[derive(Clone, Default)]
struct Recording(Arc<Mutex<(String, Vec<Option<u32>>)>>);

impl Recording {
    fn text(&self) -> String {
        self.0.lock().0.clone()
    }

    fn exits(&self) -> Vec<Option<u32>> {
        self.0.lock().1.clone()
    }

    fn wait_text(&self, needle: &str) -> bool {
        wait_until(|| self.text().contains(needle))
    }
}

impl StreamSink<PtyEvent> for Recording {
    fn send(&self, item: PtyEvent) -> Result<(), String> {
        let mut st = self.0.lock();
        match item {
            PtyEvent::Data { chunk } => st.0.push_str(&chunk),
            PtyEvent::Exit { code } => st.1.push(code),
        }
        Ok(())
    }
}

const GENEROUS: Duration = Duration::from_secs(30);

/// Poll `cond` until it holds or [`GENEROUS`] passes.
fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + GENEROUS;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    cond()
}

fn pid_exists(pid: u32) -> bool {
    let p = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[p]), true);
    sys.process(p).is_some()
}

#[allow(clippy::needless_pass_by_value)] // call sites build the argv inline
fn harness(id: &str, kind: &str, cmd: Option<Vec<String>>, session: Option<&str>) -> Harness {
    serde_json::from_value(json!({
        "id": id, "kind": kind, "name": id, "status": "idle",
        "model": "", "tokens": "0", "cmd": cmd, "sessionId": session,
    }))
    .unwrap()
}

/// Put a tool-kind harness in shell launch mode over its stored `cmd`: the
/// runner spawns the shell argv as given instead of deriving `claude ...`
/// (#752), so a stand-in shell can play a managed kind.
fn in_shell(mut h: Harness) -> Harness {
    let argv = h.cmd.clone().expect("a stored cmd");
    h.launch = Some(skein_proto::rooms::LaunchMode::Shell { argv });
    h
}

fn room(id: &str, cwd: &Path, harnesses: Vec<Harness>) -> Room {
    let active = harnesses.first().map(|h| h.id.clone()).unwrap_or_default();
    let mut v: Value = json!({
        "id": id, "name": id, "task": "", "status": "idle", "badge": 0,
        "activeHarnessId": active, "cwd": cwd.to_string_lossy(),
    });
    v["harnesses"] = serde_json::to_value(harnesses).unwrap();
    serde_json::from_value(v).unwrap()
}

struct Fx {
    _dir: TempDir,
    daemon: Daemon,
    /// The room folder every test room points at.
    work: PathBuf,
    /// Stands in for the user's home: Claude transcripts live under it.
    home: PathBuf,
}

impl Fx {
    /// A daemon built the way the app builds one (`Daemon::build` over an
    /// `InProcessTransport` of no-op seams), with an empty, loaded store.
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let work = dir.path().join("work");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let paths = Paths {
            data_dir: dir.path().join("data"),
            resource_dir: Err("no bundle in tests".to_owned()),
        };
        let env = Arc::new(spawn_env(&paths, Probe::Skip).unwrap());
        let client = Arc::new(NoClient);
        let daemon = Daemon::build(
            &paths,
            env,
            "skein.test".to_owned(),
            InProcessTransport {
                events: Arc::new(NoEvents),
                client: client.clone(),
                capture: client,
            },
        )
        .unwrap();
        daemon.claude.set_home_for_test(home.clone());
        crate::rooms::load_rooms(&daemon.db, &daemon.rooms).unwrap();
        Self {
            _dir: dir,
            daemon,
            work,
            home,
        }
    }

    fn runner(&self) -> &Arc<HarnessRunner> {
        &self.daemon.harness_runner
    }

    /// Create a room through the daemon's own write path.
    fn create(&self, room: Room) {
        self.apply(RoomMutation::CreateRoom { room });
    }

    fn apply(&self, m: RoomMutation) {
        let out = self.daemon.rooms.apply("test", vec![m]).unwrap();
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    }

    /// One room `r1` with one stand-in shell harness `h1` of `kind`.
    fn with_shell(kind: &str) -> Self {
        let fx = Self::new();
        fx.create(room(
            "r1",
            &fx.work,
            vec![harness("h1", kind, Some(shell_cmd()), None)],
        ));
        fx
    }

    fn ensure(
        &self,
        restart: bool,
        sink: Option<&Recording>,
    ) -> Result<EnsureOutcome, EnsureError> {
        self.runner().ensure_running(
            &EnsureRequest {
                room_id: "r1".to_owned(),
                harness_id: "h1".to_owned(),
                rows: 24,
                cols: 80,
                env_profile: None,
                restart,
                expect_cmd: None,
                expect_launch: None,
            },
            sink.map(|s| Box::new(s.clone()) as Box<dyn StreamSink<PtyEvent>>),
        )
    }

    /// Stand in for the xterm of a view attached from spawn: `ConPTY` waits
    /// for the host to answer its cursor query, and the daemon never answers
    /// for a PTY that has a sink. A no-op off Windows.
    fn answer_cursor_query(&self, pty_id: &str, rec: &Recording) {
        if cfg!(windows) {
            assert!(
                rec.wait_text("\x1b[6n"),
                "no cursor query; got {:?}",
                rec.text()
            );
            self.daemon.pty.write(pty_id, b"\x1b[1;1R").unwrap();
        }
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        // The processes must not outlive the test.
        for room in self.daemon.rooms.rooms() {
            for h in &room.harnesses {
                self.runner().stop_harness(&h.id);
            }
        }
    }
}
