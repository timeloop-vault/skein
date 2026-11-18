//! `open_harness` with no client attached starts the harness itself (#734).
//! The runner comes from a whole daemon built the way the app builds one,
//! and the verb's state is a separate one with no client, because a daemon
//! built over a transport always has one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use skein_proto::room_mutations::RoomMutation;

use super::client::{ClientRequests, Rect, Shot, WebviewCapture};
use super::daemon_handles::DaemonHandles;
use super::state::{AgentApiState, AgentRequest};
use super::tests::{caller_for, harness, room};
use super::verbs::{self, MailContext};
use crate::daemon::{Daemon, Probe, spawn_env};
use crate::event_sink::EventSink;
use crate::paths::Paths;
use crate::transport::InProcessTransport;

struct NoEvents;

impl EventSink for NoEvents {
    fn emit_json(&self, _event: &str, _payload: String) -> Result<(), String> {
        Ok(())
    }
}

struct NoClient;

impl ClientRequests for NoClient {
    fn is_attached(&self) -> bool {
        false
    }

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

pub(super) fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    cond()
}

/// A daemon built the way the app builds one, over a client that is never
/// there, with its rooms loaded. `dir` holds its data.
pub(super) fn build_daemon(dir: &std::path::Path) -> Daemon {
    let paths = Paths {
        data_dir: dir.join("data"),
        resource_dir: Err("no bundle in tests".to_owned()),
    };
    let env = Arc::new(spawn_env(&paths, Probe::Skip).unwrap());
    let client = Arc::new(NoClient);
    let daemon = Daemon::build(
        &paths,
        env,
        "skein.test".to_owned(),
        None,
        InProcessTransport {
            events: Arc::new(NoEvents),
            client: client.clone(),
            capture: client,
        },
    )
    .unwrap();
    crate::rooms::load_rooms(&daemon.db, &daemon.rooms).unwrap();
    daemon
}

/// A state with no client over `daemon`'s store and runner.
pub(super) fn clientless_state(daemon: &Daemon) -> AgentApiState {
    let state = AgentApiState::for_test(Arc::clone(&daemon.db));
    assert!(
        state
            .set_daemon_handles(DaemonHandles {
                rooms: Arc::clone(&daemon.rooms),
                runner: Some(Arc::clone(&daemon.harness_runner)),
                harness_config: None,
                verb_lock: tokio::sync::Mutex::new(()),
            })
            .is_ok()
    );
    assert!(!state.has_client());
    state
}

#[tokio::test(flavor = "multi_thread")]
async fn open_harness_with_no_client_leaves_the_new_harness_running() {
    let dir = tempfile::TempDir::new().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let daemon = build_daemon(dir.path());
    let mut r1 = room("r1", vec![harness("h1", "claude", "main")]);
    r1.cwd = Some(work.to_string_lossy().into_owned());
    daemon
        .rooms
        .apply("test", vec![RoomMutation::CreateRoom { room: r1 }])
        .unwrap();

    let revision_before = daemon.rooms.rooms()[0].revision;
    let state = clientless_state(&daemon);
    let caller = caller_for(&daemon.db, "r1", Some("h1"));

    let args = verbs::OpenHarnessArgs {
        room: "r1".to_owned(),
        kind: Some("byoh".to_owned()),
        agent: None,
        prompt: None,
    };
    let out = verbs::open_harness(&state, &caller, &args, &MailContext::permissive(), true).await;

    let stop = |id: &str| daemon.harness_runner.stop_harness(id);
    let out = match out {
        Ok(out) => out,
        Err(e) => panic!("open_harness failed: {e:?}"),
    };
    let revision_after = daemon
        .rooms
        .rooms()
        .into_iter()
        .find(|r| r.id == "r1")
        .map(|r| r.revision);
    assert!(
        revision_after > Some(revision_before),
        "went through the intent path"
    );
    let pty_id = daemon.harness_runner.pty_id(&out.harness_id);
    let alive = pty_id
        .as_deref()
        .is_some_and(|id| wait_until(|| daemon.pty.is_alive(id)));
    stop(&out.harness_id);
    assert!(alive, "the new harness has no live process: {pty_id:?}");
}
