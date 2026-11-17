//! `PatchRoom` / `PatchHarness` (#756): the field merge, null clearing and
//! the refusals. The revision rule is in `revision.rs`.

use serde_json::{Map, Value, json};
use skein_proto::room_commands::RoomCommand as C;

use super::{Fixture, json_of, refusals, room_with};

fn map(v: &Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

fn patch_room(rev: u64, fields: &Value) -> C {
    C::PatchRoom {
        room_id: "r1".into(),
        expected_revision: rev,
        fields: map(fields),
    }
}

fn patch_harness(rev: u64, fields: &Value) -> C {
    C::PatchHarness {
        room_id: "r1".into(),
        harness_id: "h1".into(),
        expected_revision: rev,
        fields: map(fields),
    }
}

fn fixture() -> Fixture {
    Fixture::with_rooms(&[room_with("r1", &["h1"])])
}

#[test]
fn patch_room_sets_fields_and_keeps_the_rest() {
    let f = fixture();
    let out = f.execute(
        "a",
        vec![patch_room(
            0,
            &json!({"task":"write tests","badge":3,"branch":"feat/x","todos":[{"id":"t1"}]}),
        )],
    );
    assert!(out.refused.is_empty());
    let r = f.stored_room("r1");
    assert_eq!(r.task, "write tests");
    assert_eq!(r.badge, 3);
    assert_eq!(r.branch.as_deref(), Some("feat/x"));
    assert_eq!(r.todos.as_ref().map(Vec::len), Some(1));
    assert_eq!(r.name, "room r1", "untouched field");
    assert_eq!(r.harnesses.len(), 1, "harnesses kept");
    assert_eq!(r.revision, 1);
}

#[test]
fn patch_room_null_clears_an_optional_field() {
    let f = fixture();
    f.execute("a", vec![patch_room(0, &json!({"branch":"b","cwd":"/x"}))]);
    let out = f.execute("a", vec![patch_room(1, &json!({"branch":null}))]);
    assert!(out.refused.is_empty());
    let r = f.stored_room("r1");
    assert_eq!(r.branch, None);
    assert_eq!(r.cwd.as_deref(), Some("/x"));
}

#[test]
fn patch_room_refuses_each_owned_key_and_changes_nothing() {
    for key in ["id", "harnesses", "revision"] {
        let f = fixture();
        let before = json_of(&f.stored_room("r1"));
        let out = f.execute(
            "a",
            vec![patch_room(0, &json!({"task":"also set", key: json!("x")}))],
        );
        assert_eq!(refusals(&out), [(0, "owned_field")], "{key}");
        assert_eq!(json_of(&f.stored_room("r1")), before, "{key}: not merged");
        assert!(f.events().is_empty(), "{key}");
    }
}

#[test]
fn patch_room_with_an_unparseable_result_is_invalid_patch_and_changes_nothing() {
    let f = fixture();
    let before = json_of(&f.stored_room("r1"));
    let out = f.execute(
        "a",
        vec![patch_room(
            0,
            &json!({"task":"fine","badge":"not a number"}),
        )],
    );
    assert_eq!(refusals(&out), [(0, "invalid_patch")]);
    assert_eq!(json_of(&f.stored_room("r1")), before);
    // Nulling a required field also fails to parse back.
    let out = f.execute("a", vec![patch_room(0, &json!({"name":null}))]);
    assert_eq!(refusals(&out), [(0, "invalid_patch")]);
    assert_eq!(json_of(&f.stored_room("r1")), before);
    assert!(f.events().is_empty());
}

#[test]
fn patch_room_with_no_fields_is_a_no_op() {
    let f = fixture();
    let out = f.execute("a", vec![patch_room(0, &json!({}))]);
    assert_eq!(out.refused.len() + out.revisions.len(), 0);
    assert_eq!(f.stored_room("r1").revision, 0);
    assert!(f.events().is_empty());
}

#[test]
fn patch_room_refuses_an_unknown_room() {
    let f = fixture();
    let out = f.execute(
        "a",
        vec![C::PatchRoom {
            room_id: "nope".into(),
            expected_revision: 0,
            fields: map(&json!({"task":"x"})),
        }],
    );
    assert_eq!(refusals(&out), [(0, "unknown_room")]);
}

#[test]
fn patch_harness_sets_fields_and_keeps_the_rest() {
    let f = fixture();
    let out = f.execute(
        "a",
        vec![patch_harness(
            0,
            &json!({"name":"renamed","cmd":["claude","--x"],"agent":"rev","pendingNotifications":2}),
        )],
    );
    assert!(out.refused.is_empty());
    let h = f.stored_room("r1").harnesses.remove(0);
    assert_eq!(h.name, "renamed");
    assert_eq!(h.cmd, Some(vec!["claude".to_owned(), "--x".to_owned()]));
    assert_eq!(h.agent.as_deref(), Some("rev"));
    assert_eq!(h.pending_notifications, Some(2));
    assert_eq!(h.kind, "claude");
    assert_eq!(f.stored_room("r1").revision, 1);
}

#[test]
fn patch_harness_null_clears_an_optional_field() {
    let f = fixture();
    f.execute("a", vec![patch_harness(0, &json!({"agent":"rev"}))]);
    f.execute("a", vec![patch_harness(1, &json!({"agent":null}))]);
    assert_eq!(f.stored_room("r1").harnesses[0].agent, None);
}

#[test]
fn patch_harness_refuses_each_owned_key_and_changes_nothing() {
    for key in ["id", "sessionId", "opencodeVersion"] {
        let f = fixture();
        let before = json_of(&f.stored_room("r1"));
        let out = f.execute(
            "a",
            vec![patch_harness(
                0,
                &json!({"name":"also set", key: json!("x")}),
            )],
        );
        assert_eq!(refusals(&out), [(0, "owned_field")], "{key}");
        assert_eq!(json_of(&f.stored_room("r1")), before, "{key}: not merged");
    }
    // Even a null on an owned key is a write attempt.
    let f = fixture();
    let out = f.execute("a", vec![patch_harness(0, &json!({"sessionId":null}))]);
    assert_eq!(refusals(&out), [(0, "owned_field")]);
}

#[test]
fn patch_harness_with_an_unparseable_result_is_invalid_patch_and_changes_nothing() {
    let f = fixture();
    let before = json_of(&f.stored_room("r1"));
    let out = f.execute(
        "a",
        vec![patch_harness(0, &json!({"name":"fine","cmd":"not a list"}))],
    );
    assert_eq!(refusals(&out), [(0, "invalid_patch")]);
    assert_eq!(json_of(&f.stored_room("r1")), before);
}

#[test]
fn patch_harness_with_no_fields_is_a_no_op() {
    let f = fixture();
    let out = f.execute("a", vec![patch_harness(0, &json!({}))]);
    assert_eq!(out.refused.len() + out.revisions.len(), 0);
    assert!(f.events().is_empty());
}

#[test]
fn patch_harness_refuses_kind_as_owned() {
    let f = fixture();
    let before = json_of(&f.stored_room("r1"));
    let out = f.execute("a", vec![patch_harness(0, &json!({"kind":"opencode"}))]);
    assert_eq!(refusals(&out), [(0, "owned_field")]);
    assert_eq!(json_of(&f.stored_room("r1")), before);
    assert!(f.events().is_empty());
}

#[test]
fn patches_with_an_unknown_key_are_invalid_and_change_nothing() {
    let f = fixture();
    let before = json_of(&f.stored_room("r1"));
    for fields in [
        json!({"nmae":"x"}),
        json!({"task":"fine","nmae":"x"}),
        json!({"nmae":null}),
    ] {
        let out = f.execute("a", vec![patch_room(0, &fields)]);
        assert_eq!(refusals(&out), [(0, "invalid_patch")], "room {fields}");
        let out = f.execute("a", vec![patch_harness(0, &fields)]);
        assert_eq!(refusals(&out), [(0, "invalid_patch")], "harness {fields}");
        assert_eq!(json_of(&f.stored_room("r1")), before, "{fields}");
    }
    assert_eq!(f.stored_room("r1").revision, 0);
    assert!(f.events().is_empty());
}

#[test]
fn setting_an_unset_optional_lands_and_clearing_an_absent_one_is_a_no_op() {
    let f = fixture();
    let out = f.execute(
        "a",
        vec![patch_room(
            0,
            &json!({"attention":true,"todos":[{"id":"t"}],"repoRoot":"/x"}),
        )],
    );
    assert!(out.refused.is_empty());
    let r = f.stored_room("r1");
    assert_eq!(r.attention, Some(true));
    assert_eq!(r.todos.as_ref().map(Vec::len), Some(1));
    assert_eq!(r.revision, 1);

    let out = f.execute(
        "a",
        vec![patch_harness(
            1,
            &json!({"envProfile":"p","designEntry":"e","hermesProfile":"h"}),
        )],
    );
    assert!(out.refused.is_empty());
    let h = f.stored_room("r1").harnesses.remove(0);
    assert_eq!(h.env_profile.as_deref(), Some("p"));
    assert_eq!(h.design_entry.as_deref(), Some("e"));
    assert_eq!(f.stored_room("r1").revision, 2);

    // Already-absent optionals: null is a no-op, neither refused nor bumped.
    let f = fixture();
    let out = f.execute("a", vec![patch_room(0, &json!({"closedBy":null}))]);
    assert!(out.refused.is_empty() && out.revisions.is_empty());
    let out = f.execute("a", vec![patch_harness(0, &json!({"remote":null}))]);
    assert!(out.refused.is_empty() && out.revisions.is_empty());
    assert_eq!(f.stored_room("r1").revision, 0);
    assert!(f.events().is_empty());
}

#[test]
fn a_harness_patch_of_a_structural_key_is_persisted_at_once() {
    let f = fixture();
    f.execute("a", vec![patch_harness(0, &json!({"name":"renamed"}))]);
    let on_disk = f.disk_room("r1").unwrap();
    assert_eq!(on_disk.harnesses[0].name, "renamed", "no flush needed");
}

#[test]
fn a_harness_patch_of_a_non_structural_key_waits_for_flush() {
    let f = fixture();
    f.execute("a", vec![patch_harness(0, &json!({"tokens":"42"}))]);
    assert_eq!(f.stored_room("r1").harnesses[0].tokens, "42");
    assert_eq!(f.disk_room("r1").unwrap().harnesses[0].tokens, "0");
    f.store.flush().unwrap();
    assert_eq!(f.disk_room("r1").unwrap().harnesses[0].tokens, "42");
}

/// The wire names of the pub fields of `struct <name>` in skein-proto.
fn proto_fields(src: &str, name: &str) -> Vec<String> {
    let start = src.find(&format!("pub struct {name} {{")).unwrap();
    let body = &src[start..];
    let body = &body[..body.find("\n}").unwrap()];
    body.lines()
        .skip(1)
        .filter_map(|l| l.trim().strip_prefix("pub "))
        .map(|l| {
            let snake = l.split(':').next().unwrap();
            let mut out = String::new();
            let mut up = false;
            for c in snake.chars() {
                if c == '_' {
                    up = true;
                } else if up {
                    out.push(c.to_ascii_uppercase());
                    up = false;
                } else {
                    out.push(c);
                }
            }
            out
        })
        .collect()
}

#[test]
fn the_known_key_lists_match_the_proto_structs() {
    let src = include_str!("../../../../skein-proto/src/rooms.rs");
    for (name, list) in [
        ("Room", crate::room_store::commands::ROOM_KEYS),
        ("Harness", crate::room_store::commands::HARNESS_KEYS),
    ] {
        let mut want = proto_fields(src, name);
        let mut have: Vec<String> = list.iter().map(|s| (*s).to_owned()).collect();
        want.sort();
        have.sort();
        assert_eq!(
            have, want,
            "{name}: update ROOM_KEYS/HARNESS_KEYS in room_store/commands.rs"
        );
    }
}

#[test]
fn patch_harness_refuses_an_unknown_harness_and_an_unknown_room() {
    let f = fixture();
    let out = f.execute(
        "a",
        vec![
            C::PatchHarness {
                room_id: "r1".into(),
                harness_id: "ghost".into(),
                expected_revision: 0,
                fields: map(&json!({"name":"x"})),
            },
            C::PatchHarness {
                room_id: "nope".into(),
                harness_id: "h1".into(),
                expected_revision: 0,
                fields: map(&json!({"name":"x"})),
            },
        ],
    );
    assert_eq!(
        refusals(&out),
        [(0, "unknown_harness"), (1, "unknown_room")]
    );
}
