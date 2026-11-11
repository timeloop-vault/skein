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
