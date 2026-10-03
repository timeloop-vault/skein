//! #491: the Claude Code version seed and live change detector.

use super::cli_version::{latest_cli_version_event, live_cli_version_event};
use super::tests::{append_lines, drain, drain_brief, test_manager};
use super::*;
use std::sync::mpsc;
use tempfile::TempDir;

fn row(version: &str, ts: &str) -> String {
    format!(
        r#"{{"type":"attachment","version":"{version}","timestamp":"{ts}","message":{{"content":[]}}}}"#
    )
}

fn versions(events: &[ClaudeEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            ClaudeEvent::CliVersion { version, .. } => Some(version.clone()),
            _ => None,
        })
        .collect()
}

fn attach_over(path: &std::path::Path) -> mpsc::Receiver<ClaudeEvent> {
    let (tx, rx) = mpsc::channel();
    let manager = test_manager();
    manager
        .attach_at(
            "harness-1",
            path.to_path_buf(),
            move |event| {
                let _ = tx.send(event);
            },
            None,
        )
        .unwrap();
    // Keep the manager (and its watcher) alive for the test's lifetime.
    std::mem::forget(manager);
    rx
}

#[test]
fn attach_seeds_the_latest_version_once() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    append_lines(
        &path,
        &[
            &row("2.1.280", "2026-05-15T21:00:00.000Z"),
            r#"{"type":"user","message":{"content":"no version here"}}"#,
            &row("2.1.288", "2026-05-15T22:00:00.000Z"),
            r#"{"type":"last-prompt"}"#,
        ],
    );
    let rx = attach_over(&path);
    let events = drain(&rx);
    assert_eq!(versions(&events), vec!["2.1.288".to_owned()], "{events:?}");
    let seeded = events
        .iter()
        .find_map(|e| match e {
            ClaudeEvent::CliVersion { timestamp_ms, .. } => *timestamp_ms,
            _ => None,
        })
        .expect("timestamp");
    assert_eq!(
        Some(seeded),
        skein_harness::time::parse_iso8601_ms("2026-05-15T22:00:00.000Z")
    );
}

#[test]
fn first_live_row_after_attach_emits_even_when_the_version_matches_the_seed() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    append_lines(&path, &[&row("2.1.288", "2026-05-15T22:00:00.000Z")]);
    let rx = attach_over(&path);
    assert_eq!(versions(&drain(&rx)), vec!["2.1.288".to_owned()]);

    append_lines(&path, &[&row("2.1.288", "2026-05-15T22:05:00.000Z")]);
    assert_eq!(versions(&drain(&rx)), vec!["2.1.288".to_owned()]);
}

#[test]
fn live_change_emits_and_live_repeat_is_suppressed() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    let rx = attach_over(&path);
    assert_eq!(versions(&drain_brief(&rx)), Vec::<String>::new());

    append_lines(&path, &[&row("2.1.288", "2026-05-15T22:00:00.000Z")]);
    assert_eq!(versions(&drain(&rx)), vec!["2.1.288".to_owned()]);

    append_lines(&path, &[&row("2.1.288", "2026-05-15T22:01:00.000Z")]);
    assert_eq!(
        versions(&drain_brief(&rx)),
        Vec::<String>::new(),
        "repeat must not emit"
    );

    append_lines(&path, &[&row("2.1.290", "2026-05-15T22:02:00.000Z")]);
    assert_eq!(versions(&drain(&rx)), vec!["2.1.290".to_owned()]);
}

#[test]
fn helpers_ignore_sidechain_rows_and_missing_versions() {
    let sidechain = serde_json::json!({"isSidechain": true, "version": "2.1.1"});
    let none = serde_json::json!({"type": "user"});
    let mut last = None;
    assert!(live_cli_version_event(&sidechain, &mut last).is_none());
    assert!(live_cli_version_event(&none, &mut last).is_none());
    assert!(last.is_none());
    assert!(latest_cli_version_event(&sidechain.to_string()).is_none());
    assert!(latest_cli_version_event("").is_none());
}
