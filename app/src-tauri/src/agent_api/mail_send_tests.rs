//! The mailbox (#327): `send_message` and `read_messages`, their guards and caps. Also hosts the mail fixtures the other mail files share.

use super::auth::Caller;
use super::tests::{caller_for, fixture, harness, room, save};
use super::verbs::{self, MailPolicy, ReadMessagesArgs, SendMessageArgs, VerbError};
use crate::db::Database;

// ── the mailbox (issue #327) ─────────────────────────────────────────

fn always_sees_mcp(_kind: &str, _agent: &str, _cwd: &str) -> bool {
    true
}

fn never_sees_mcp(_kind: &str, _agent: &str, _cwd: &str) -> bool {
    false
}

fn send_with(
    db: &Database,
    caller: &Caller,
    to: &str,
    body: &str,
    policy: MailPolicy,
    agent_sees_mcp: verbs::AgentSeesMcp,
) -> Result<verbs::SendMessageOut, VerbError> {
    verbs::send_message(
        db,
        caller,
        &SendMessageArgs {
            to: to.to_owned(),
            body: body.to_owned(),
        },
        policy,
        agent_sees_mcp,
        None,
    )
}

pub(super) fn send(
    db: &Database,
    caller: &Caller,
    to: &str,
    body: &str,
) -> Result<verbs::SendMessageOut, VerbError> {
    send_with(
        db,
        caller,
        to,
        body,
        MailPolicy::permissive(),
        always_sees_mcp,
    )
}

pub(super) fn read(
    db: &Database,
    caller: &Caller,
    include_read: Option<bool>,
) -> Result<verbs::ReadMessagesOut, VerbError> {
    verbs::read_messages(
        db,
        caller,
        &ReadMessagesArgs { include_read },
        MailPolicy::permissive(),
    )
}

#[test]
fn a_sent_message_shows_up_in_the_recipients_mailbox() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "claude", "main")]),
        ],
    );
    let sender = caller_for(&f.db, "r1", Some("h1"));
    let out = send(&f.db, &sender, "h2", "hello from r1").unwrap();
    assert_eq!(out.to_room_id, "r2");
    assert_eq!(out.to_harness_id, "h2");
    assert_eq!(out.to_room_name, "room r2");
    assert_eq!(out.to_harness_name, "main");

    let reader = caller_for(&f.db, "r2", Some("h2"));
    let inbox = read(&f.db, &reader, None).unwrap();
    assert_eq!(inbox.messages.len(), 1);
    assert_eq!(inbox.newly_marked_read, 1);
    let m = &inbox.messages[0];
    assert_eq!(m.body, "hello from r1");
    assert_eq!(m.from_room_id, "r1");
    assert_eq!(m.from_room_name.as_deref(), Some("room r1"));
    assert_eq!(m.from_harness_id.as_deref(), Some("h1"));
    assert_eq!(m.from_harness_name.as_deref(), Some("claude · main"));
    assert!(m.read_ms.is_some());
}

/// #356's `list_rooms` reads this directly off the `Database`, not
/// through a verb — worth its own test independent of that plumbing.
#[test]
fn latest_message_from_room_picks_the_newest_and_only_toward_the_right_recipient() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "claude", "main")]),
            room("r3", vec![harness("h3", "claude", "main")]),
        ],
    );
    let child = caller_for(&f.db, "r1", Some("h1"));
    send(&f.db, &child, "r2", "status: first").unwrap();
    send(&f.db, &child, "r3", "status: to the wrong director").unwrap();
    send(&f.db, &child, "r2", "status: second, and newer").unwrap();

    let latest =
        f.db.latest_message_from_room("r1", "r2")
            .unwrap()
            .expect("r1 has sent r2 something");
    assert_eq!(latest.body, "status: second, and newer");

    assert!(
        f.db.latest_message_from_room("r2", "r1").unwrap().is_none(),
        "the direction matters — r2 never sent r1 anything"
    );
}

#[test]
fn unread_messages_come_back_oldest_first() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "claude", "main")]),
        ],
    );
    let sender = caller_for(&f.db, "r1", Some("h1"));
    send(&f.db, &sender, "h2", "first").unwrap();
    send(&f.db, &sender, "h2", "second").unwrap();
    send(&f.db, &sender, "h2", "third").unwrap();

    let reader = caller_for(&f.db, "r2", Some("h2"));
    let inbox = read(&f.db, &reader, None).unwrap();
    let bodies: Vec<&str> = inbox.messages.iter().map(|m| m.body.as_str()).collect();
    assert_eq!(bodies, vec!["first", "second", "third"]);
}

#[test]
fn reading_marks_messages_read_so_a_second_read_is_empty() {
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

    let reader = caller_for(&f.db, "r2", Some("h2"));
    let first = read(&f.db, &reader, None).unwrap();
    assert_eq!(first.messages.len(), 1);

    let second = read(&f.db, &reader, None).unwrap();
    assert!(second.messages.is_empty());
    assert_eq!(second.newly_marked_read, 0);
}

#[test]
fn include_read_returns_the_whole_history_without_double_counting() {
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

    let reader = caller_for(&f.db, "r2", Some("h2"));
    read(&f.db, &reader, None).unwrap();

    let history = read(&f.db, &reader, Some(true)).unwrap();
    assert_eq!(history.messages.len(), 1);
    assert_eq!(
        history.newly_marked_read, 0,
        "already read before this call — must not count again"
    );
}

#[test]
fn sending_to_a_room_id_resolves_to_its_lead_harness_skipping_a_shell() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room(
                "r2",
                vec![
                    harness("h-shell", "byoh", "shell"),
                    harness("h-claude", "claude", "main"),
                ],
            ),
        ],
    );
    let sender = caller_for(&f.db, "r1", Some("h1"));
    let out = send(&f.db, &sender, "r2", "hi").unwrap();
    assert_eq!(
        out.to_harness_id, "h-claude",
        "the shell harness must be skipped, not chosen as lead"
    );
}

#[test]
fn messaging_off_refuses_both_verbs() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "claude", "main")]),
        ],
    );
    let off = MailPolicy {
        messaging_enabled: false,
        ..MailPolicy::permissive()
    };
    let sender = caller_for(&f.db, "r1", Some("h1"));
    let err = send_with(&f.db, &sender, "h2", "hi", off, always_sees_mcp).unwrap_err();
    assert!(matches!(err, VerbError::Refused(_)));
    assert!(err.message().contains("turned off"));

    let reader = caller_for(&f.db, "r2", Some("h2"));
    let err = verbs::read_messages(&f.db, &reader, &ReadMessagesArgs::default(), off).unwrap_err();
    assert!(matches!(err, VerbError::Refused(_)));
    assert!(err.message().contains("turned off"));
}

#[test]
fn an_empty_body_is_refused() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "claude", "main")]),
        ],
    );
    let sender = caller_for(&f.db, "r1", Some("h1"));
    let err = send(&f.db, &sender, "h2", "").unwrap_err();
    assert!(matches!(err, VerbError::Refused(_)));
}

#[test]
fn an_oversize_body_is_refused_with_the_cap_named() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "claude", "main")]),
        ],
    );
    let sender = caller_for(&f.db, "r1", Some("h1"));
    let huge = "x".repeat(65 * 1024);
    let err = send(&f.db, &sender, "h2", &huge).unwrap_err();
    assert!(matches!(err, VerbError::Refused(_)));
    assert!(err.message().contains("65536"), "{}", err.message());
}

#[test]
fn an_unknown_target_is_not_found() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let sender = caller_for(&f.db, "r1", Some("h1"));
    let err = send(&f.db, &sender, "nowhere", "hi").unwrap_err();
    assert!(matches!(err, VerbError::NotFound(_)));
}

#[test]
fn an_archived_target_room_is_refused_by_harness_id_and_by_room_id() {
    let f = fixture();
    let mut r2 = room("r2", vec![harness("h2", "claude", "main")]);
    r2.archived = Some(1);
    save(
        &f.db,
        &[room("r1", vec![harness("h1", "claude", "main")]), r2],
    );
    let sender = caller_for(&f.db, "r1", Some("h1"));

    let by_harness = send(&f.db, &sender, "h2", "hi").unwrap_err();
    assert!(matches!(by_harness, VerbError::Refused(_)));
    assert!(by_harness.message().contains("archived"));

    let by_room = send(&f.db, &sender, "r2", "hi").unwrap_err();
    assert!(by_room.message().contains("archived"));
}

#[test]
fn a_shell_harness_cannot_read_mail() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h-shell", "byoh", "shell")]),
        ],
    );
    let sender = caller_for(&f.db, "r1", Some("h1"));
    let err = send(&f.db, &sender, "h-shell", "hi").unwrap_err();
    assert!(matches!(err, VerbError::Refused(_)));
    assert!(
        err.message().contains("cannot read messages"),
        "{}",
        err.message()
    );
}

#[test]
fn injection_off_for_a_kind_refuses_that_kind() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "claude", "main")]),
        ],
    );
    let policy = MailPolicy {
        claude_injected: false,
        ..MailPolicy::permissive()
    };
    let sender = caller_for(&f.db, "r1", Some("h1"));
    let err = send_with(&f.db, &sender, "h2", "hi", policy, always_sees_mcp).unwrap_err();
    assert!(matches!(err, VerbError::Refused(_)));
    assert!(err.message().contains("injection"), "{}", err.message());
}

#[test]
fn an_agent_that_hides_mcp_tools_refuses_the_send() {
    let f = fixture();
    let mut h2 = harness("h2", "claude", "main");
    h2.agent = Some("narrow".to_owned());
    let mut r2 = room("r2", vec![h2]);
    // The agent-lookup seam only runs when there is a cwd to look
    // under; the value never has to resolve to a real directory here
    // because `never_sees_mcp` never reads it.
    r2.cwd = Some("/wherever".to_owned());
    save(
        &f.db,
        &[room("r1", vec![harness("h1", "claude", "main")]), r2],
    );
    let sender = caller_for(&f.db, "r1", Some("h1"));
    let err = send_with(
        &f.db,
        &sender,
        "h2",
        "hi",
        MailPolicy::permissive(),
        never_sees_mcp,
    )
    .unwrap_err();
    assert!(matches!(err, VerbError::Refused(_)));
    assert!(err.message().contains("allowlist"), "{}", err.message());
}

#[test]
fn a_room_with_no_mail_reading_harness_is_refused() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h-shell", "byoh", "shell")]),
        ],
    );
    let sender = caller_for(&f.db, "r1", Some("h1"));
    let err = send(&f.db, &sender, "r2", "hi").unwrap_err();
    assert!(matches!(err, VerbError::Refused(_)));
    assert!(err.message().contains("no harness"), "{}", err.message());
}

#[test]
fn read_messages_needs_a_harness_header() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", None);
    let err = read(&f.db, &caller, None).unwrap_err();
    assert!(matches!(err, VerbError::Refused(_)));
    assert!(err.message().contains("X-Skein-Harness"));
}

#[test]
fn a_room_is_rate_limited_to_thirty_sends_a_minute() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "claude", "main")]),
        ],
    );
    let sender = caller_for(&f.db, "r1", Some("h1"));
    for i in 0..30 {
        send(&f.db, &sender, "h2", &format!("msg {i}")).unwrap();
    }
    let err = send(&f.db, &sender, "h2", "one too many").unwrap_err();
    assert!(matches!(err, VerbError::Refused(_)));
    assert!(err.message().contains("rate limit"));
    assert!(err.message().contains("30"));
}

#[test]
fn a_receiving_harness_caps_at_a_hundred_unread() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "claude", "main")]),
        ],
    );
    // Seeded directly, far outside the rate-limit window, so this test
    // isolates the unread cap from the send-rate cap above.
    for i in 0..100 {
        f.db.insert_harness_message(&crate::db::HarnessMessageRow {
            id: format!("seed-{i}"),
            room_id: "r2".to_owned(),
            harness_id: "h2".to_owned(),
            from_room_id: "r1".to_owned(),
            from_harness_id: Some("h1".to_owned()),
            body: "seed".to_owned(),
            created_ms: 1,
            read_ms: None,
        })
        .unwrap();
    }
    let sender = caller_for(&f.db, "r1", Some("h1"));
    let err = send(&f.db, &sender, "h2", "one more").unwrap_err();
    assert!(matches!(err, VerbError::Refused(_)));
    assert!(err.message().contains("100"), "{}", err.message());
}

#[test]
fn same_millisecond_messages_read_back_in_insertion_order() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    // Ids are deliberately out of alphabetical order so a tie-break on
    // `id` (a random UUID) would reorder them; only a tie-break on
    // rowid preserves the insertion order the spec requires.
    for id in ["z-third", "a-first", "m-second"] {
        f.db.insert_harness_message(&crate::db::HarnessMessageRow {
            id: id.to_owned(),
            room_id: "r1".to_owned(),
            harness_id: "h1".to_owned(),
            from_room_id: "r1".to_owned(),
            from_harness_id: None,
            body: id.to_owned(),
            created_ms: 42,
            read_ms: None,
        })
        .unwrap();
    }
    let all = f.db.all_harness_messages("r1", "h1").unwrap();
    let ids: Vec<&str> = all.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["z-third", "a-first", "m-second"]);
}
