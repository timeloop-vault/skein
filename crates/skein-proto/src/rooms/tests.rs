use serde_json::json;

use super::{Harness, LaunchMode, Room};

fn blob(extra: &serde_json::Value) -> serde_json::Value {
    let mut v = json!({
        "id": "h1", "kind": "claude", "name": "c", "status": "idle",
        "model": "m", "tokens": "0", "cmd": ["claude"]
    });
    if let (Some(base), Some(add)) = (v.as_object_mut(), extra.as_object()) {
        base.extend(add.clone());
    }
    v
}

#[test]
fn an_old_blob_without_launch_parses_as_none_and_stays_without_the_key() {
    let h: Harness = serde_json::from_value(blob(&json!({}))).expect("parse");
    assert_eq!(h.launch, None);
    let back = serde_json::to_value(&h).expect("serialize");
    assert!(back.get("launch").is_none());
}

#[test]
fn the_shell_form_round_trips() {
    let h: Harness = serde_json::from_value(blob(&json!({
        "launch": {"mode": "shell", "argv": ["pwsh.exe"]}
    })))
    .expect("parse");
    assert_eq!(
        h.launch,
        Some(LaunchMode::Shell {
            argv: vec!["pwsh.exe".to_owned()]
        })
    );
    let back = serde_json::to_value(&h).expect("serialize");
    assert_eq!(
        back["launch"],
        json!({"mode": "shell", "argv": ["pwsh.exe"]})
    );
}

#[test]
fn the_tool_form_is_just_a_mode_tag() {
    assert_eq!(
        serde_json::to_value(LaunchMode::Tool).expect("serialize"),
        json!({"mode": "tool"})
    );
    let back: LaunchMode = serde_json::from_value(json!({"mode": "tool"})).expect("parse");
    assert_eq!(back, LaunchMode::Tool);
}

#[test]
fn a_room_blob_without_revision_parses_as_zero() {
    let v = json!({"id":"r1","name":"r","task":"","status":"idle","badge":0,
        "harnesses":[],"activeHarnessId":""});
    let r: Room = serde_json::from_value(v).expect("parse");
    assert_eq!(r.revision, 0);
    let back = serde_json::to_value(&r).expect("serialize");
    assert_eq!(back["revision"], 0, "always serialized");
}

#[test]
fn a_room_revision_round_trips_under_its_camel_case_key() {
    let v = json!({"id":"r1","name":"r","task":"","status":"idle","badge":0,
        "harnesses":[],"activeHarnessId":"","revision":7});
    let r: Room = serde_json::from_value(v).expect("parse");
    assert_eq!(r.revision, 7);
    assert_eq!(serde_json::to_value(&r).unwrap()["revision"], 7);
}
