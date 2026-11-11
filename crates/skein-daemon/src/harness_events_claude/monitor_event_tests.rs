//! Epic #439 field report: a Monitor's events (enqueue + dequeue + a
//! status-less task-notification user row + a text-only end of turn)
//! must never read as the Monitor ending.

use super::background_tests::{t445_append, t445_ends, t445_live, t445_starts};
use super::resync_tests::t425_tick;
use super::*;
use tempfile::TempDir;

#[test]
fn t439_monitor_events_start_once_and_never_end() {
    // The tick's sweep runs on the wall clock and the fixture is stamped
    // 2026-01-01, so stretch the 30 min timeout out of reach; the deadline
    // itself is covered in skein-harness.
    let raw = include_str!(
        "../../../../crates/skein-harness/src/claude/background/fixtures/monitor_events.jsonl"
    );
    assert_eq!(
        raw.matches("1800000").count(),
        2,
        "fixture no longer carries the 30 min timeout twice (tool_use + result)"
    );
    let fixture = raw.replace("1800000", "9999999999999");
    let rows: Vec<String> = fixture.lines().map(String::from).collect();
    assert_eq!(rows.len(), 2 + 4 * 4);

    let dir = TempDir::new().unwrap();
    let (state, path, _sub, db) = t445_live(&dir, &[]);
    let mut events = Vec::new();
    // Feed each repetition on its own tick, as the tail would see them.
    t445_append(&path, &rows[..2]);
    events.extend(t425_tick(&state));
    for rep in rows[2..].chunks(4) {
        t445_append(&path, rep);
        let tick = t425_tick(&state);
        assert!(t445_ends(&tick).is_empty(), "{tick:?}");
        events.extend(tick);
    }

    let starts = t445_starts(&events);
    assert!(
        matches!(
            starts.as_slice(),
            [ClaudeEvent::BackgroundStart {
                task_kind: BackgroundKind::Monitor,
                initial: false,
                ..
            }]
        ),
        "{events:?}"
    );
    assert!(t445_ends(&events).is_empty(), "{events:?}");
    let prompts = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                ClaudeEvent::UserPrompt {
                    task_notification: true
                }
            )
        })
        .count();
    assert_eq!(prompts, 4, "{events:?}");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
        "{events:?}"
    );
    assert_eq!(
        super::background_tests::t445_end_rows(&db),
        Vec::<serde_json::Value>::new()
    );
}
