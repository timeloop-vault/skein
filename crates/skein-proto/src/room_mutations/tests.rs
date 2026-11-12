use super::*;
use serde_json::{Value, json};

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

fn roundtrip<T: Serialize + for<'de> Deserialize<'de>>(v: &T) -> Value {
    let a = serde_json::to_value(v).unwrap();
    let back: T = serde_json::from_value(a.clone()).unwrap();
    assert_eq!(serde_json::to_value(&back).unwrap(), a);
    a
}

fn tag_and_keys(v: &Value) -> (String, Vec<String>) {
    let o = v.as_object().unwrap();
    let mut keys: Vec<String> = o.keys().cloned().collect();
    keys.sort();
    (o["type"].as_str().unwrap().to_string(), keys)
}

#[test]
fn mutation_variants_use_the_contract_tags_and_fields() {
    let cases: Vec<(RoomMutation, &str, Vec<&str>)> = vec![
        (
            RoomMutation::CreateRoom { room: room() },
            "createRoom",
            vec!["room", "type"],
        ),
        (
            RoomMutation::UpdateRoom { room: room() },
            "updateRoom",
            vec!["room", "type"],
        ),
        (
            RoomMutation::RemoveRoom {
                room_id: "r1".into(),
            },
            "removeRoom",
            vec!["roomId", "type"],
        ),
        (
            RoomMutation::ReorderRooms {
                room_ids: vec!["a".into()],
            },
            "reorderRooms",
            vec!["roomIds", "type"],
        ),
        (
            RoomMutation::PutHarness {
                room_id: "r1".into(),
                harness: harness(),
            },
            "putHarness",
            vec!["harness", "roomId", "type"],
        ),
        (
            RoomMutation::RemoveHarness {
                room_id: "r1".into(),
                harness_id: "h1".into(),
            },
            "removeHarness",
            vec!["harnessId", "roomId", "type"],
        ),
        (
            RoomMutation::ReorderHarnesses {
                room_id: "r1".into(),
                harness_ids: vec!["h1".into()],
            },
            "reorderHarnesses",
            vec!["harnessIds", "roomId", "type"],
        ),
    ];
    for (m, tag, keys) in cases {
        let (t, k) = tag_and_keys(&roundtrip(&m));
        assert_eq!(t, tag);
        assert_eq!(k, keys, "{tag}");
    }
}

#[test]
fn change_variants_use_the_contract_tags_and_fields() {
    let cases: Vec<(RoomChange, &str, Vec<&str>)> = vec![
        (
            RoomChange::RoomPut { room: room() },
            "roomPut",
            vec!["room", "type"],
        ),
        (
            RoomChange::RoomRemoved {
                room_id: "r1".into(),
            },
            "roomRemoved",
            vec!["roomId", "type"],
        ),
        (
            RoomChange::RoomsOrder {
                room_ids: vec!["r1".into()],
            },
            "roomsOrder",
            vec!["roomIds", "type"],
        ),
    ];
    for (c, tag, keys) in cases {
        let (t, k) = tag_and_keys(&roundtrip(&c));
        assert_eq!(t, tag);
        assert_eq!(k, keys, "{tag}");
    }
}

#[test]
fn envelope_types_are_camel_case() {
    let ev = RoomsChanged {
        origin: "c1".into(),
        changes: vec![RoomChange::RoomRemoved {
            room_id: "r1".into(),
        }],
    };
    assert_eq!(
        roundtrip(&ev),
        json!({"origin":"c1","changes":[{"type":"roomRemoved","roomId":"r1"}]})
    );
    let out = RoomsApplyOutcome {
        skipped: vec![SkippedMutation {
            index: 2,
            reason: "exists".into(),
        }],
    };
    assert_eq!(
        roundtrip(&out),
        json!({"skipped":[{"index":2,"reason":"exists"}]})
    );
}

#[test]
fn deserializes_the_frontend_shape() {
    let m: RoomMutation =
        serde_json::from_value(json!({"type":"removeHarness","roomId":"r","harnessId":"h"}))
            .unwrap();
    assert!(matches!(m, RoomMutation::RemoveHarness { .. }));
}
