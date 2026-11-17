use serde_json::{Value, json};

use super::*;

fn room() -> Room {
    serde_json::from_value(
        json!({"id":"r1","name":"r","task":"","status":"idle","badge":0,
        "harnesses":[],"activeHarnessId":""}),
    )
    .unwrap()
}

fn harness() -> Harness {
    serde_json::from_value(json!({"id":"h1","kind":"claude","name":"h","status":"idle",
        "model":"","tokens":"0"}))
    .unwrap()
}

fn fields(v: &Value) -> serde_json::Map<String, Value> {
    v.as_object().unwrap().clone()
}

/// Serialize, check it parses back to the same JSON, return the JSON.
fn wire(cmd: &RoomCommand) -> Value {
    let a = serde_json::to_value(cmd).unwrap();
    let back: RoomCommand = serde_json::from_value(a.clone()).unwrap();
    assert_eq!(serde_json::to_value(&back).unwrap(), a);
    a
}

fn tag_and_keys(v: &Value) -> (String, Vec<String>) {
    let o = v.as_object().unwrap();
    let mut keys: Vec<String> = o.keys().cloned().collect();
    keys.sort();
    (o["type"].as_str().unwrap().to_owned(), keys)
}

#[test]
fn rename_room_has_the_documented_shape() {
    let v = wire(&RoomCommand::RenameRoom {
        room_id: "r".into(),
        name: "n".into(),
    });
    assert_eq!(v, json!({"type":"renameRoom","roomId":"r","name":"n"}));
}

#[test]
fn every_variant_uses_a_camel_case_tag_and_camel_case_fields() {
    let cases: Vec<(RoomCommand, &str, Vec<&str>)> = vec![
        (
            RoomCommand::CreateRoom { room: room() },
            "createRoom",
            vec!["room", "type"],
        ),
        (
            RoomCommand::ArchiveRoom {
                room_id: "r".into(),
                closed_by: Some(ClosedByRef {
                    room_id: "c".into(),
                    harness_id: None,
                }),
            },
            "archiveRoom",
            vec!["closedBy", "roomId", "type"],
        ),
        (
            RoomCommand::UnarchiveRoom {
                room_id: "r".into(),
            },
            "unarchiveRoom",
            vec!["roomId", "type"],
        ),
        (
            RoomCommand::ReorderRooms {
                room_ids: vec!["a".into()],
            },
            "reorderRooms",
            vec!["roomIds", "type"],
        ),
        (
            RoomCommand::AddHarness {
                room_id: "r".into(),
                harness: harness(),
                activate: true,
            },
            "addHarness",
            vec!["activate", "harness", "roomId", "type"],
        ),
        (
            RoomCommand::CloseHarness {
                room_id: "r".into(),
                harness_id: "h".into(),
            },
            "closeHarness",
            vec!["harnessId", "roomId", "type"],
        ),
        (
            RoomCommand::RestartHarness {
                room_id: "r".into(),
                harness_id: "h".into(),
            },
            "restartHarness",
            vec!["harnessId", "roomId", "type"],
        ),
        (
            RoomCommand::SetHarnessAgent {
                room_id: "r".into(),
                harness_id: "h".into(),
                agent: Some("a".into()),
            },
            "setHarnessAgent",
            vec!["agent", "harnessId", "roomId", "type"],
        ),
        (
            RoomCommand::ReorderHarnesses {
                room_id: "r".into(),
                harness_ids: vec!["h".into()],
            },
            "reorderHarnesses",
            vec!["harnessIds", "roomId", "type"],
        ),
        (
            RoomCommand::PatchRoom {
                room_id: "r".into(),
                expected_revision: 3,
                fields: fields(&json!({"task":"t"})),
            },
            "patchRoom",
            vec!["expectedRevision", "fields", "roomId", "type"],
        ),
        (
            RoomCommand::PatchHarness {
                room_id: "r".into(),
                harness_id: "h".into(),
                expected_revision: 3,
                fields: fields(&json!({"name":"x"})),
            },
            "patchHarness",
            vec!["expectedRevision", "fields", "harnessId", "roomId", "type"],
        ),
    ];
    for (cmd, tag, keys) in cases {
        let (got_tag, got_keys) = tag_and_keys(&wire(&cmd));
        assert_eq!(got_tag, tag);
        assert_eq!(got_keys, keys, "{tag}");
    }
}

#[test]
fn archive_room_without_closed_by_parses_as_none() {
    let cmd: RoomCommand =
        serde_json::from_value(json!({"type":"archiveRoom","roomId":"r"})).unwrap();
    assert!(matches!(
        cmd,
        RoomCommand::ArchiveRoom {
            closed_by: None,
            ..
        }
    ));
}

#[test]
fn set_harness_agent_without_agent_parses_as_none() {
    let cmd: RoomCommand =
        serde_json::from_value(json!({"type":"setHarnessAgent","roomId":"r","harnessId":"h"}))
            .unwrap();
    assert!(matches!(
        cmd,
        RoomCommand::SetHarnessAgent { agent: None, .. }
    ));
}

#[test]
fn add_harness_without_activate_parses_as_false() {
    let cmd: RoomCommand = serde_json::from_value(
        json!({"type":"addHarness","roomId":"r","harness":serde_json::to_value(harness()).unwrap()}),
    )
    .unwrap();
    assert!(matches!(
        cmd,
        RoomCommand::AddHarness {
            activate: false,
            ..
        }
    ));
}

#[test]
fn closed_by_ref_omits_a_missing_harness_id() {
    let v = serde_json::to_value(ClosedByRef {
        room_id: "c".into(),
        harness_id: None,
    })
    .unwrap();
    assert_eq!(v, json!({"roomId":"c"}));
    let back: ClosedByRef = serde_json::from_value(json!({"roomId":"c"})).unwrap();
    assert_eq!(back.harness_id, None);
    let v = serde_json::to_value(ClosedByRef {
        room_id: "c".into(),
        harness_id: Some("h".into()),
    })
    .unwrap();
    assert_eq!(v, json!({"roomId":"c","harnessId":"h"}));
}

#[test]
fn a_patch_null_survives_the_wire_as_null() {
    let cmd: RoomCommand = serde_json::from_value(json!({
        "type":"patchRoom","roomId":"r","expectedRevision":1,"fields":{"branch":null}
    }))
    .unwrap();
    let RoomCommand::PatchRoom { fields, .. } = cmd else {
        panic!("not a patchRoom");
    };
    assert_eq!(fields.get("branch"), Some(&Value::Null));
}

#[test]
fn execute_outcome_has_the_documented_shape() {
    let out = RoomsExecuteOutcome {
        refused: vec![RefusedCommand {
            index: 2,
            reason: "exists".into(),
        }],
        revisions: vec![RoomRevision {
            room_id: "r".into(),
            revision: 4,
        }],
    };
    let v = serde_json::to_value(&out).unwrap();
    assert_eq!(
        v,
        json!({
            "refused":[{"index":2,"reason":"exists"}],
            "revisions":[{"roomId":"r","revision":4}]
        })
    );
    let back: RoomsExecuteOutcome = serde_json::from_value(v).unwrap();
    assert_eq!(back.revisions, out.revisions);
    assert_eq!(back.refused[0].index, 2);
}
