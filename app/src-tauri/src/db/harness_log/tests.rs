use super::*;
use crate::db::test_support::fresh_db;

#[test]
fn record_then_query_by_harness_returns_event() {
    let (_dir, db) = fresh_db();
    db.record_harness_event("h1", "r1", "running", "waiting", 1_000, true, Some("l2c1"))
        .unwrap();
    let events = db.recent_harness_events_by_harness("h1", 0, 10).unwrap();
    assert_eq!(events.len(), 1);
    let e = &events[0];
    assert_eq!(e.harness_id, "h1");
    assert_eq!(e.room_id, "r1");
    assert_eq!(e.from_phase, "running");
    assert_eq!(e.to_phase, "waiting");
    assert_eq!(e.timestamp_ms, 1_000);
    assert!(e.has_user_input);
    assert_eq!(e.source.as_deref(), Some("l2c1"));
}

#[test]
fn query_excludes_events_at_or_before_since_ms() {
    let (_dir, db) = fresh_db();
    for ts in [100, 200, 300, 400] {
        db.record_harness_event("h1", "r1", "running", "idle", ts, false, None)
            .unwrap();
    }
    let events = db.recent_harness_events_by_harness("h1", 200, 10).unwrap();
    // Strict > since_ms — caller passes the last-seen timestamp
    // and wants only events newer than that.
    let timestamps: Vec<i64> = events.iter().map(|e| e.timestamp_ms).collect();
    assert_eq!(timestamps, vec![400, 300]);
}

#[test]
fn query_is_scoped_by_harness_id() {
    let (_dir, db) = fresh_db();
    db.record_harness_event("h1", "r1", "running", "idle", 100, false, None)
        .unwrap();
    db.record_harness_event("h2", "r1", "running", "idle", 200, false, None)
        .unwrap();
    db.record_harness_event("h1", "r1", "idle", "running", 300, false, None)
        .unwrap();
    let h1 = db.recent_harness_events_by_harness("h1", 0, 10).unwrap();
    assert_eq!(h1.len(), 2);
    assert!(h1.iter().all(|e| e.harness_id == "h1"));
}

#[test]
fn query_by_room_returns_all_harnesses_in_that_room() {
    let (_dir, db) = fresh_db();
    db.record_harness_event("h1", "r1", "running", "idle", 100, false, None)
        .unwrap();
    db.record_harness_event("h2", "r1", "running", "idle", 200, false, None)
        .unwrap();
    db.record_harness_event("h3", "r2", "running", "idle", 300, false, None)
        .unwrap();
    let r1 = db.recent_harness_events_by_room("r1", 0, 10).unwrap();
    assert_eq!(r1.len(), 2);
    assert!(r1.iter().all(|e| e.room_id == "r1"));
}

#[test]
fn query_respects_limit() {
    let (_dir, db) = fresh_db();
    for ts in 0..50 {
        db.record_harness_event("h1", "r1", "running", "idle", ts, false, None)
            .unwrap();
    }
    let events = db.recent_harness_events_by_harness("h1", -1, 5).unwrap();
    assert_eq!(events.len(), 5);
    // Newest first — last ts is the largest.
    assert_eq!(events[0].timestamp_ms, 49);
    assert_eq!(events[4].timestamp_ms, 45);
}

#[test]
fn has_user_input_round_trips_correctly() {
    let (_dir, db) = fresh_db();
    db.record_harness_event("h1", "r1", "spawning", "running", 100, false, None)
        .unwrap();
    db.record_harness_event("h2", "r1", "running", "waiting", 200, true, None)
        .unwrap();
    let events = db.recent_harness_events_by_room("r1", 0, 10).unwrap();
    // Newest-first ordering means h2 comes back first.
    assert!(events[0].has_user_input);
    assert!(!events[1].has_user_input);
}

#[test]
fn null_source_round_trips_as_none() {
    let (_dir, db) = fresh_db();
    db.record_harness_event("h1", "r1", "running", "idle", 100, false, None)
        .unwrap();
    let events = db.recent_harness_events_by_harness("h1", 0, 10).unwrap();
    assert!(events[0].source.is_none());
}

// ── harness_actions (issue #80) ───────────────────────────────

#[test]
fn action_record_then_query_by_harness_returns_row() {
    let (_dir, db) = fresh_db();
    let payload = r#"{"tool":"bash","input":{"command":"ls"}}"#;
    db.record_harness_action(
        "h1",
        "r1",
        1_000,
        action_kind::TOOL_CALL,
        payload,
        Some("l2c1"),
    )
    .unwrap();
    let actions = db.recent_harness_actions_by_harness("h1", 0, 10).unwrap();
    assert_eq!(actions.len(), 1);
    let a = &actions[0];
    assert_eq!(a.harness_id, "h1");
    assert_eq!(a.room_id, "r1");
    assert_eq!(a.timestamp_ms, 1_000);
    assert_eq!(a.kind, "tool_call");
    assert_eq!(a.payload, payload);
    assert_eq!(a.source.as_deref(), Some("l2c1"));
}

#[test]
fn action_query_excludes_rows_at_or_before_since_ms() {
    let (_dir, db) = fresh_db();
    for ts in [100, 200, 300, 400] {
        db.record_harness_action("h1", "r1", ts, action_kind::PATCH, "{}", None)
            .unwrap();
    }
    let actions = db.recent_harness_actions_by_harness("h1", 200, 10).unwrap();
    let timestamps: Vec<i64> = actions.iter().map(|a| a.timestamp_ms).collect();
    assert_eq!(timestamps, vec![400, 300]);
}

#[test]
fn action_query_is_scoped_by_harness_id() {
    let (_dir, db) = fresh_db();
    db.record_harness_action("h1", "r1", 100, action_kind::TOOL_CALL, "{}", None)
        .unwrap();
    db.record_harness_action("h2", "r1", 200, action_kind::TOOL_CALL, "{}", None)
        .unwrap();
    db.record_harness_action("h1", "r1", 300, action_kind::PATCH, "{}", None)
        .unwrap();
    let h1 = db.recent_harness_actions_by_harness("h1", 0, 10).unwrap();
    assert_eq!(h1.len(), 2);
    assert!(h1.iter().all(|a| a.harness_id == "h1"));
}

#[test]
fn action_query_by_room_returns_all_harnesses_in_that_room() {
    let (_dir, db) = fresh_db();
    db.record_harness_action("h1", "r1", 100, action_kind::TOOL_CALL, "{}", None)
        .unwrap();
    db.record_harness_action("h2", "r1", 200, action_kind::TOOL_CALL, "{}", None)
        .unwrap();
    db.record_harness_action("h3", "r2", 300, action_kind::TOOL_CALL, "{}", None)
        .unwrap();
    let r1 = db.recent_harness_actions_by_room("r1", 0, 10).unwrap();
    assert_eq!(r1.len(), 2);
    assert!(r1.iter().all(|a| a.room_id == "r1"));
}

#[test]
fn action_query_by_room_and_kind_filters_other_kinds_out() {
    let (_dir, db) = fresh_db();
    db.record_harness_action(
        "h1",
        "r1",
        100,
        action_kind::PLAN_CHANGE,
        r#"{"n":1}"#,
        None,
    )
    .unwrap();
    db.record_harness_action("h1", "r1", 200, action_kind::TOOL_CALL, "{}", None)
        .unwrap();
    db.record_harness_action(
        "h2",
        "r1",
        300,
        action_kind::PLAN_CHANGE,
        r#"{"n":2}"#,
        None,
    )
    .unwrap();
    let plans = db
        .recent_harness_actions_by_room_and_kind("r1", action_kind::PLAN_CHANGE, 0, 10)
        .unwrap();
    assert_eq!(plans.len(), 2);
    assert!(plans.iter().all(|a| a.kind == "plan_change"));
    // Newest first.
    assert_eq!(plans[0].timestamp_ms, 300);
    assert_eq!(plans[1].timestamp_ms, 100);
}

#[test]
fn action_query_respects_limit() {
    let (_dir, db) = fresh_db();
    for ts in 0..50 {
        db.record_harness_action("h1", "r1", ts, action_kind::TOOL_CALL, "{}", None)
            .unwrap();
    }
    let actions = db.recent_harness_actions_by_harness("h1", -1, 5).unwrap();
    assert_eq!(actions.len(), 5);
    assert_eq!(actions[0].timestamp_ms, 49);
    assert_eq!(actions[4].timestamp_ms, 45);
}

#[test]
fn batch_record_lands_every_row_in_one_transaction() {
    let (_dir, db) = fresh_db();
    let rows = vec![
        NewHarnessAction {
            timestamp_ms: 100,
            kind: action_kind::TOOL_CALL,
            payload: r#"{"n":1}"#.into(),
            source: None,
        },
        NewHarnessAction {
            timestamp_ms: 200,
            kind: action_kind::PATCH,
            payload: r#"{"n":2}"#.into(),
            source: Some("l2c1".into()),
        },
        NewHarnessAction {
            timestamp_ms: 300,
            kind: action_kind::TOOL_CALL,
            payload: r#"{"n":3}"#.into(),
            source: None,
        },
    ];
    let inserted = db.record_harness_actions("h1", "r1", &rows).unwrap();
    assert_eq!(inserted, 3);
    let actions = db.recent_harness_actions_by_harness("h1", 0, 10).unwrap();
    assert_eq!(actions.len(), 3);
    assert!(
        actions
            .iter()
            .all(|a| a.harness_id == "h1" && a.room_id == "r1")
    );
    // Newest first.
    assert_eq!(actions[0].payload, r#"{"n":3}"#);
    assert_eq!(actions[1].source.as_deref(), Some("l2c1"));
    assert_eq!(actions[2].payload, r#"{"n":1}"#);
}

#[test]
fn batch_record_of_empty_rows_is_a_no_op() {
    let (_dir, db) = fresh_db();
    let inserted = db.record_harness_actions("h1", "r1", &[]).unwrap();
    assert_eq!(inserted, 0);
    assert!(
        db.recent_harness_actions_by_harness("h1", -1, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn action_payload_is_stored_verbatim_including_unicode_and_quotes() {
    // The DB layer must not parse / re-serialize / escape payloads
    // beyond what sqlite needs — adapters write JSON, consumers
    // read the same bytes back.
    let (_dir, db) = fresh_db();
    let payload = r#"{"text":"hello \"world\" — café 🌮","nested":{"k":[1,2,3]}}"#;
    db.record_harness_action("h1", "r1", 100, action_kind::AWAY_SUMMARY, payload, None)
        .unwrap();
    let actions = db.recent_harness_actions_by_harness("h1", 0, 10).unwrap();
    assert_eq!(actions[0].payload, payload);
}

#[test]
fn action_query_orders_same_ms_rows_by_id_desc() {
    // Two actions written in the same millisecond must come back
    // in insertion order (newest first), so the timeline doesn't
    // flicker between Skein restarts.
    let (_dir, db) = fresh_db();
    db.record_harness_action("h1", "r1", 100, action_kind::TOOL_CALL, r#"{"n":1}"#, None)
        .unwrap();
    db.record_harness_action("h1", "r1", 100, action_kind::TOOL_CALL, r#"{"n":2}"#, None)
        .unwrap();
    db.record_harness_action("h1", "r1", 100, action_kind::TOOL_CALL, r#"{"n":3}"#, None)
        .unwrap();
    let actions = db.recent_harness_actions_by_harness("h1", 0, 10).unwrap();
    assert_eq!(actions.len(), 3);
    assert_eq!(actions[0].payload, r#"{"n":3}"#);
    assert_eq!(actions[2].payload, r#"{"n":1}"#);
}

#[test]
fn action_null_source_round_trips_as_none() {
    let (_dir, db) = fresh_db();
    db.record_harness_action("h1", "r1", 100, action_kind::TOOL_CALL, "{}", None)
        .unwrap();
    let actions = db.recent_harness_actions_by_harness("h1", 0, 10).unwrap();
    assert!(actions[0].source.is_none());
}

#[test]
fn action_kind_constants_match_persisted_strings() {
    // Lock the v1 vocabulary so accidental renames trigger a
    // failing test (consumers read these strings directly from
    // the DB; renaming would orphan historical rows).
    assert_eq!(action_kind::TOOL_CALL, "tool_call");
    assert_eq!(action_kind::PLAN_CHANGE, "plan_change");
    assert_eq!(action_kind::PATCH, "patch");
    assert_eq!(action_kind::PR_LINK, "pr_link");
    assert_eq!(action_kind::QUEUE_OP, "queue_op");
    assert_eq!(action_kind::EDITED_TEXT_FILE, "edited_text_file");
    assert_eq!(action_kind::SLASH_COMMAND, "slash_command");
    assert_eq!(action_kind::AWAY_SUMMARY, "away_summary");
    assert_eq!(action_kind::TURN_DURATION, "turn_duration");
    assert_eq!(action_kind::API_ERROR, "api_error");
    assert_eq!(action_kind::TURN_COST, "turn_cost");
    assert_eq!(action_kind::PERMISSION_MODE, "permission_mode");
    assert_eq!(action_kind::AI_TITLE, "ai_title");
    assert_eq!(action_kind::BRIDGE_STATUS, "bridge_status");
    assert_eq!(action_kind::USER_PROMPT, "user_prompt");
    assert_eq!(action_kind::COMPACTION, "compaction");
    assert_eq!(action_kind::REASONING, "reasoning");
}
