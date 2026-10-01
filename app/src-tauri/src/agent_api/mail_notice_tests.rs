//! Mail notices: the unread summary (#329) and the `message_in` / `message_out` `harness_actions` rows.

use serde_json::json;

use super::mail_send_tests::send;
use super::tests::{caller_for, fixture, harness, room, save};
use super::verbs::{self};

// ── unread mail summary, issue #329 ────────────────────────────────

#[test]
fn mail_unread_counts_and_names_distinct_senders_oldest_first() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "claude", "main")]),
            room("r3", vec![harness("h3", "claude", "main")]),
        ],
    );
    let r1 = caller_for(&f.db, "r1", Some("h1"));
    let r2 = caller_for(&f.db, "r2", Some("h2"));
    send(&f.db, &r2, "h1", "from r2 first").unwrap();
    send(
        &f.db,
        &r1,
        "h1",
        "self-talk, not counted as unread for h1... wait",
    )
    .unwrap();
    let r3 = caller_for(&f.db, "r3", Some("h3"));
    send(&f.db, &r3, "h1", "from r3").unwrap();
    send(&f.db, &r2, "h1", "from r2 again").unwrap();

    let messages = f.db.unread_harness_messages("r1", "h1").unwrap();
    let rooms = f.db.all_rooms().unwrap();
    let summary = verbs::unread_mail(&messages, &rooms);

    assert_eq!(summary.count, 4);
    assert_eq!(
        summary.from_room_names,
        vec!["room r2", "room r1", "room r3"],
        "distinct names, first-seen order — r2 must not repeat for its second message"
    );
}

#[test]
fn mail_unread_does_not_mark_anything_read() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "claude", "main")]),
        ],
    );
    let sender = caller_for(&f.db, "r1", Some("h1"));
    send(&f.db, &sender, "h2", "hi").unwrap();

    let messages = f.db.unread_harness_messages("r2", "h2").unwrap();
    let rooms = f.db.all_rooms().unwrap();
    let before = verbs::unread_mail(&messages, &rooms);
    assert_eq!(before.count, 1);

    // Calling it again must see the same message — nothing was marked.
    let messages_again = f.db.unread_harness_messages("r2", "h2").unwrap();
    let after = verbs::unread_mail(&messages_again, &rooms);
    assert_eq!(after.count, 1);
}

#[test]
fn mail_unread_falls_back_to_the_room_id_when_the_sender_room_is_gone() {
    let f = fixture();
    save(&f.db, &[room("r2", vec![harness("h2", "claude", "main")])]);
    f.db.insert_harness_message(&crate::db::HarnessMessageRow {
        id: "m1".to_owned(),
        room_id: "r2".to_owned(),
        harness_id: "h2".to_owned(),
        from_room_id: "r-vanished".to_owned(),
        from_harness_id: None,
        body: "hi".to_owned(),
        created_ms: 1,
        read_ms: None,
    })
    .unwrap();

    let messages = f.db.unread_harness_messages("r2", "h2").unwrap();
    let rooms = f.db.all_rooms().unwrap();
    let summary = verbs::unread_mail(&messages, &rooms);
    assert_eq!(summary.from_room_names, vec!["r-vanished"]);
}

// ── message_in / message_out harness_actions rows, issue #329 ──────

#[test]
fn a_send_writes_one_message_in_row_for_the_recipient_and_one_message_out_row_for_the_sender() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "claude", "main")]),
        ],
    );
    let sender = caller_for(&f.db, "r1", Some("h1"));
    let out = send(&f.db, &sender, "h2", "hello").unwrap();

    let in_rows = f.db.recent_harness_actions_by_room("r2", 0, 100).unwrap();
    let in_row = in_rows
        .iter()
        .find(|a| a.kind == crate::db::action_kind::MESSAGE_IN)
        .expect("expected exactly one message_in row for the recipient");
    assert_eq!(in_row.harness_id, "h2");
    assert_eq!(in_row.room_id, "r2");
    let in_payload: serde_json::Value = serde_json::from_str(&in_row.payload).unwrap();
    assert_eq!(in_payload["message_id"], json!(out.message_id));
    assert_eq!(in_payload["from_room_id"], json!("r1"));
    assert_eq!(in_payload["to_room_id"], json!("r2"));
    assert_eq!(in_payload["to_harness_id"], json!("h2"));
    assert!(
        in_payload.get("body").is_none(),
        "no message body belongs in a feed payload"
    );

    let out_rows = f.db.recent_harness_actions_by_room("r1", 0, 100).unwrap();
    let out_row = out_rows
        .iter()
        .find(|a| a.kind == crate::db::action_kind::MESSAGE_OUT)
        .expect("expected exactly one message_out row for the sender");
    assert_eq!(out_row.harness_id, "h1");
    assert_eq!(out_row.room_id, "r1");
    assert_eq!(out_row.timestamp_ms, in_row.timestamp_ms);

    assert!(
        !in_rows
            .iter()
            .any(|a| a.kind == crate::db::action_kind::MESSAGE_OUT),
        "the recipient's room must not also get the sender's row"
    );
    assert!(
        !out_rows
            .iter()
            .any(|a| a.kind == crate::db::action_kind::MESSAGE_IN),
        "the sender's room must not also get the recipient's row"
    );
}

#[test]
fn a_send_with_no_caller_harness_records_an_empty_harness_id_on_the_out_row() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![]),
            room("r2", vec![harness("h2", "claude", "main")]),
        ],
    );
    let sender = caller_for(&f.db, "r1", None);
    send(&f.db, &sender, "h2", "hi").unwrap();

    let out_rows = f.db.recent_harness_actions_by_room("r1", 0, 100).unwrap();
    let out_row = out_rows
        .iter()
        .find(|a| a.kind == crate::db::action_kind::MESSAGE_OUT)
        .expect("expected a message_out row even with no attributed harness");
    assert_eq!(out_row.harness_id, "");
}
