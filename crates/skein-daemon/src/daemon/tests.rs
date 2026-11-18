use std::sync::Arc;

use super::*;
use crate::event_sink::EventSink;

struct NoopSink;

impl EventSink for NoopSink {
    fn emit_json(&self, _event: &str, _payload: String) -> Result<(), String> {
        Ok(())
    }
}

fn sink() -> SharedEventSink {
    Arc::new(NoopSink)
}

#[test]
fn builds_from_paths_without_a_tauri_app() {
    let tmp = tempfile::TempDir::new().unwrap();
    let data_dir = tmp.path().join("data");
    let resource_dir = tmp.path().join("res");
    std::fs::create_dir_all(&resource_dir).unwrap();
    let paths = Paths {
        data_dir: data_dir.clone(),
        resource_dir: Ok(resource_dir),
    };

    let env = spawn_env(&paths, Probe::Skip).unwrap();
    assert_eq!(env.data_dir, data_dir);
    let state = SharedState::build(&paths, sink()).unwrap();

    assert!(data_dir.join("skein.db").is_file());
    // A resource dir without the bundle resolves, and reports what is missing.
    let status = state.harness_config.status(None);
    assert!(status.claude_plugin.is_none());
    assert!(status.error.is_some());
}

#[derive(Default)]
struct Recording(std::sync::Mutex<Vec<(String, String)>>);

impl EventSink for Recording {
    fn emit_json(&self, event: &str, payload: String) -> Result<(), String> {
        self.0.lock().unwrap().push((event.to_owned(), payload));
        Ok(())
    }
}

struct NoClient;

impl crate::agent_api::client::ClientRequests for NoClient {
    fn is_attached(&self) -> bool {
        false
    }

    fn send(&self, _request: &crate::agent_api::state::AgentRequest) -> Result<(), String> {
        Err("no client".to_owned())
    }
}

impl crate::agent_api::client::WebviewCapture for NoClient {
    fn capture(
        &self,
        _rect: crate::agent_api::client::Rect,
        _scale: f64,
        _done: Box<dyn FnOnce(Result<crate::agent_api::client::Shot, String>) + Send>,
    ) -> Result<(), String> {
        Err("no client".to_owned())
    }
}

#[test]
fn an_event_from_a_daemon_built_component_arrives_stamped() {
    let tmp = tempfile::TempDir::new().unwrap();
    let paths = Paths {
        data_dir: tmp.path().join("data"),
        resource_dir: Err("no bundle in tests".to_owned()),
    };
    let env = Arc::new(spawn_env(&paths, Probe::Skip).unwrap());
    let raw = Arc::new(Recording::default());
    let client = Arc::new(NoClient);
    let daemon = Daemon::build(
        &paths,
        env,
        "skein.test".to_owned(),
        InProcessTransport {
            events: raw.clone(),
            client: client.clone(),
            capture: client,
        },
    )
    .unwrap();
    crate::rooms::load_rooms(&daemon.db, &daemon.rooms).unwrap();
    let room: skein_proto::rooms::Room = serde_json::from_value(serde_json::json!({
        "id": "r1", "name": "r1", "task": "", "status": "idle",
        "badge": 0, "harnesses": [], "activeHarnessId": ""
    }))
    .unwrap();
    daemon
        .rooms
        .apply(
            "test",
            vec![skein_proto::room_mutations::RoomMutation::CreateRoom { room }],
        )
        .unwrap();

    let seen = raw.0.lock().unwrap();
    let (name, payload) = seen
        .iter()
        .find(|(n, _)| n == skein_proto::room_mutations::ROOMS_CHANGED_EVENT)
        .expect("the store's event reached the transport sink");
    let v: serde_json::Value = serde_json::from_str(payload).unwrap();
    assert_eq!(v["seq"], 1, "{name}: {payload}");
    assert_eq!(daemon.event_seq.current(), 1);
    assert_eq!(daemon.rooms.snapshot().1, 1);
}

#[test]
fn an_unresolvable_resource_dir_is_not_fatal() {
    let tmp = tempfile::TempDir::new().unwrap();
    let paths = Paths {
        data_dir: tmp.path().to_path_buf(),
        resource_dir: Err("no such place".to_string()),
    };

    let state = SharedState::build(&paths, sink()).unwrap();

    let error = state.harness_config.status(None).error.unwrap();
    assert_eq!(error, "resource directory unavailable: no such place");
    assert!(paths.db_path().is_file());
}
