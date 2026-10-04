use tempfile::TempDir;

use super::{Database, Harness, Room};

pub(super) fn fresh_db() -> (TempDir, Database) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.db");
    let db = Database::open(&path).unwrap();
    (dir, db)
}

pub(super) fn room(id: &str) -> Room {
    Room {
        id: id.into(),
        name: format!("room {id}"),
        task: String::new(),
        status: "idle".into(),
        badge: 0,
        harnesses: Vec::new(),
        active_harness_id: String::new(),
        cwd: None,
        branch: None,
        repo: None,
        archived: None,
        repo_root: None,
        attention: None,
        created_by: None,
        closed_by: None,
        retired: None,
        repo_identity: None,
    }
}

pub(super) fn harness(id: &str) -> Harness {
    Harness {
        id: id.into(),
        kind: "claude".into(),
        name: id.into(),
        status: "running".into(),
        model: String::new(),
        tokens: "0".into(),
        live: None,
        cmd: None,
        cwd: None,
        session_id: None,
        agent: None,
        design_entry: None,
        design_device: None,
        pending_notifications: None,
        created_by: None,
        shell_claim: None,
    }
}
