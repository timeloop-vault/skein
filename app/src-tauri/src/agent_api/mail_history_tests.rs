//! Mail history (#364): `message_history` direction, filters, paging and scope.

use super::auth::Caller;
use super::mail_send_tests::{read, send};
use super::tests::{caller_for, fixture, harness, room, save};
use super::verbs::{
    self, HistoryDirection, HistorySince, MailPolicy, MessageHistoryArgs, VerbError,
};
use crate::db::Database;

// ── mail history, issue #364 ────────────────────────────────────────

fn history(
    db: &Database,
    caller: &Caller,
    args: &MessageHistoryArgs,
) -> Result<verbs::MessageHistoryOut, VerbError> {
    verbs::message_history(db, caller, args, MailPolicy::permissive())
}

#[test]
fn history_defaults_to_the_inbox_and_never_marks_anything_read() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "claude", "main")]),
        ],
    );
    let sender = caller_for(&f.db, "r1", Some("h1"));
    send(&f.db, &sender, "h2", "hello").unwrap();

    let reader = caller_for(&f.db, "r2", Some("h2"));
    let out = history(&f.db, &reader, &MessageHistoryArgs::default()).unwrap();
    assert_eq!(out.messages.len(), 1);
    let m = &out.messages[0];
    assert_eq!(m.direction, "inbound");
    assert_eq!(m.body, "hello");
    assert!(
        m.read_ms.is_none(),
        "must still read as unread — history never marks anything read"
    );
    assert!(!out.has_more);
    assert_eq!(
        f.db.unread_harness_message_count("r2", "h2").unwrap(),
        1,
        "message_history must leave the unread count untouched"
    );

    // The strongest proof this call changed nothing: a real read_messages
    // call afterwards still finds it unread.
    let inbox = read(&f.db, &reader, None).unwrap();
    assert_eq!(inbox.newly_marked_read, 1);
}

#[test]
fn direction_out_shows_the_rooms_own_outbox_not_its_inbox() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "claude", "main")]),
        ],
    );
    let sender = caller_for(&f.db, "r1", Some("h1"));
    send(&f.db, &sender, "h2", "outbound body").unwrap();

    let out = history(
        &f.db,
        &sender,
        &MessageHistoryArgs {
            direction: Some(HistoryDirection::Out),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(out.messages.len(), 1);
    let m = &out.messages[0];
    assert_eq!(m.direction, "outbound");
    assert_eq!(m.to_room_id, "r2");
    assert_eq!(m.to_harness_id, "h2");
    assert_eq!(m.body, "outbound body");

    // direction "out" alone needs no X-Skein-Harness (#364's brief is
    // explicit about this).
    let headless = caller_for(&f.db, "r1", None);
    let headless_out = history(
        &f.db,
        &headless,
        &MessageHistoryArgs {
            direction: Some(HistoryDirection::Out),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(headless_out.messages.len(), 1);

    // The default direction ("in") sees none of this — the sender's own
    // inbox is empty.
    let default_view = history(&f.db, &sender, &MessageHistoryArgs::default()).unwrap();
    assert!(default_view.messages.is_empty());
}

#[test]
fn direction_out_with_no_caller_harness_still_sees_a_message_to_a_sibling_in_the_same_room() {
    let f = fixture();
    save(
        &f.db,
        &[room(
            "r1",
            vec![
                harness("h1", "claude", "main"),
                harness("h2", "claude", "second"),
            ],
        )],
    );
    let sender = caller_for(&f.db, "r1", Some("h1"));
    send(&f.db, &sender, "h2", "to my sibling").unwrap();

    // A headless caller (no X-Skein-Harness) asking for its room's own
    // outbox must still see a message to a harness in the SAME room —
    // regression for a SQL bug where `NOT (... AND harness_id =
    // :harness)` evaluated to NULL (dropping the row) once `:harness`
    // itself was NULL, rather than the NULL-safe "no caller harness, so
    // it cannot be this row's recipient" it should mean.
    let headless = caller_for(&f.db, "r1", None);
    let out = history(
        &f.db,
        &headless,
        &MessageHistoryArgs {
            direction: Some(HistoryDirection::Out),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(out.messages.len(), 1);
    assert_eq!(out.messages[0].direction, "outbound");
    assert_eq!(out.messages[0].to_room_id, "r1");
    assert_eq!(out.messages[0].to_harness_id, "h2");
}

#[test]
fn a_sibling_harnesss_message_is_classified_inbound_not_double_counted() {
    let f = fixture();
    save(
        &f.db,
        &[room(
            "r1",
            vec![
                harness("h1", "claude", "main"),
                harness("h2", "claude", "second"),
            ],
        )],
    );
    let sibling = caller_for(&f.db, "r1", Some("h2"));
    send(&f.db, &sibling, "h1", "from my sibling").unwrap();

    let caller = caller_for(&f.db, "r1", Some("h1"));
    let both = history(
        &f.db,
        &caller,
        &MessageHistoryArgs {
            direction: Some(HistoryDirection::Both),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(both.messages.len(), 1, "must not be counted twice");
    assert_eq!(both.messages[0].direction, "inbound");

    let out_only = history(
        &f.db,
        &caller,
        &MessageHistoryArgs {
            direction: Some(HistoryDirection::Out),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        out_only.messages.is_empty(),
        "h1 never sent this — it must not read as h1's own outbound"
    );
}

#[test]
fn with_filters_both_directions_by_room_or_harness_id() {
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
    send(&f.db, &r1, "h2", "to r2").unwrap();
    send(&f.db, &r1, "h3", "to r3").unwrap();

    // Outbound, filtered by room id.
    let out = history(
        &f.db,
        &r1,
        &MessageHistoryArgs {
            direction: Some(HistoryDirection::Out),
            with: Some("r2".to_owned()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(out.messages.len(), 1);
    assert_eq!(out.messages[0].to_room_id, "r2");

    // Inbound, filtered by harness id.
    let r3 = caller_for(&f.db, "r3", Some("h3"));
    send(&f.db, &r3, "h2", "also to r2, from r3").unwrap();
    let r2 = caller_for(&f.db, "r2", Some("h2"));
    let inbound = history(
        &f.db,
        &r2,
        &MessageHistoryArgs {
            with: Some("h1".to_owned()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(inbound.messages.len(), 1);
    assert_eq!(inbound.messages[0].from_harness_id.as_deref(), Some("h1"));
}

#[test]
fn since_and_limit_page_forward_with_has_more() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "claude", "main")]),
        ],
    );
    // Seeded directly with controlled timestamps — real sends inside one
    // test can land in the same millisecond, which would make the
    // ms-cursor assertions below flaky.
    for i in 0..5i64 {
        f.db.insert_harness_message(&crate::db::HarnessMessageRow {
            id: format!("m{i}"),
            room_id: "r2".to_owned(),
            harness_id: "h2".to_owned(),
            from_room_id: "r1".to_owned(),
            from_harness_id: Some("h1".to_owned()),
            body: format!("msg {i}"),
            created_ms: 100 + i,
            read_ms: None,
        })
        .unwrap();
    }
    let reader = caller_for(&f.db, "r2", Some("h2"));

    let page1 = history(
        &f.db,
        &reader,
        &MessageHistoryArgs {
            limit: Some(2),
            ..Default::default()
        },
    )
    .unwrap();
    let bodies: Vec<&str> = page1.messages.iter().map(|m| m.body.as_str()).collect();
    assert_eq!(bodies, vec!["msg 0", "msg 1"]);
    assert!(page1.has_more);

    let cursor = page1.messages[1].id.clone();
    let page2 = history(
        &f.db,
        &reader,
        &MessageHistoryArgs {
            limit: Some(2),
            since: Some(HistorySince::MessageId(cursor)),
            ..Default::default()
        },
    )
    .unwrap();
    let bodies2: Vec<&str> = page2.messages.iter().map(|m| m.body.as_str()).collect();
    assert_eq!(bodies2, vec!["msg 2", "msg 3"]);
    assert!(page2.has_more);

    let page3 = history(
        &f.db,
        &reader,
        &MessageHistoryArgs {
            since: Some(HistorySince::Ms(103)),
            ..Default::default()
        },
    )
    .unwrap();
    let bodies3: Vec<&str> = page3.messages.iter().map(|m| m.body.as_str()).collect();
    assert_eq!(bodies3, vec!["msg 4"]);
    assert!(!page3.has_more);
}

#[test]
fn another_rooms_outbox_and_another_harnesss_inbox_are_never_visible() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room(
                "r2",
                vec![harness("h2a", "claude", "a"), harness("h2b", "claude", "b")],
            ),
            room("r3", vec![harness("h3", "claude", "main")]),
        ],
    );
    let r3 = caller_for(&f.db, "r3", Some("h3"));
    send(&f.db, &r3, "h2a", "to h2a").unwrap();

    // r1 asking for its own outbound history must never see r3's.
    let r1 = caller_for(&f.db, "r1", Some("h1"));
    let r1_out = history(
        &f.db,
        &r1,
        &MessageHistoryArgs {
            direction: Some(HistoryDirection::Out),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(r1_out.messages.is_empty());

    // h2b, a sibling of the actual recipient h2a, must never see h2a's
    // own inbox, even though they share a room.
    let h2b = caller_for(&f.db, "r2", Some("h2b"));
    let h2b_in = history(&f.db, &h2b, &MessageHistoryArgs::default()).unwrap();
    assert!(h2b_in.messages.is_empty());
}

#[test]
fn a_since_message_id_outside_the_callers_scope_refuses() {
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
    let sent = send(&f.db, &r1, "h2", "r1 to r2").unwrap();

    // r3 never sent or received anything with r1 or r2 — this message id
    // is outside its scope in either direction.
    let r3 = caller_for(&f.db, "r3", Some("h3"));
    let err = history(
        &f.db,
        &r3,
        &MessageHistoryArgs {
            since: Some(HistorySince::MessageId(sent.message_id)),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(matches!(err, VerbError::Refused(_)));
    assert!(err.message().contains("since"));
}

#[test]
fn messaging_off_refuses_message_history_too() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let off = MailPolicy {
        messaging_enabled: false,
        ..MailPolicy::permissive()
    };
    let err =
        verbs::message_history(&f.db, &caller, &MessageHistoryArgs::default(), off).unwrap_err();
    assert!(matches!(err, VerbError::Refused(_)));
    assert!(err.message().contains("turned off"));
}

#[test]
fn message_history_needs_a_harness_header_for_in_or_both_but_not_out() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let headless = caller_for(&f.db, "r1", None);

    let err = history(&f.db, &headless, &MessageHistoryArgs::default()).unwrap_err();
    assert!(matches!(err, VerbError::Refused(_)));
    assert!(err.message().contains("X-Skein-Harness"));

    let err_both = history(
        &f.db,
        &headless,
        &MessageHistoryArgs {
            direction: Some(HistoryDirection::Both),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(matches!(err_both, VerbError::Refused(_)));

    let ok = history(
        &f.db,
        &headless,
        &MessageHistoryArgs {
            direction: Some(HistoryDirection::Out),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(ok.messages.is_empty());
}
