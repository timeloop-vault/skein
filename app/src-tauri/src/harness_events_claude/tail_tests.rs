//! Main-file tailing basics: seek, carry, attach, detach, heartbeat and UTF-8 stall guards.

use super::adapter::TailState;
use super::background_tasks::BackgroundState;
use super::paths::DirId;
use super::tests::{drain, drain_brief, make_adapter, test_manager};
use super::tick::{
    HEARTBEAT_INTERVAL, UTF8_STALL_WARN_THRESHOLD, should_heartbeat, should_warn_utf8_stall, tick,
};
use super::*;
use parking_lot::Mutex;
use skein_harness::claude::local_command::LocalCommandTracker;
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;
use std::{fs, thread};
use tempfile::TempDir;

#[test]
fn detach_reports_whether_an_adapter_was_removed() {
    let dir = TempDir::new().unwrap();
    let (mgr, _path, _rx) = make_adapter(&dir);

    assert!(
        !mgr.detach("no-such-harness"),
        "detach of an unknown id should report false"
    );
    assert!(
        mgr.detach("harness-1"),
        "detach of the attached id should report true"
    );
    assert!(
        !mgr.detach("harness-1"),
        "detaching the same id twice should report false the second time"
    );
}

#[test]
fn pre_existing_file_seeks_to_eof_and_only_emits_new_lines() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    {
        let mut f = fs::File::create(&path).unwrap();
        // Historical end_turn — we should NOT replay this.
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();
    }

    let (tx, rx) = mpsc::channel();
    let manager = test_manager();
    manager
        .attach_at(
            "h1",
            path.clone(),
            move |e| {
                tx.send(e).unwrap();
            },
            None,
        )
        .unwrap();

    // Append a fresh end_turn row. Scope the file handle so the
    // OS closes it before we drain — macOS FSEvents holds modify
    // events on an open handle until close, even after fsync.
    {
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();
    }

    let events = drain(&rx);
    let prompt_count = events
        .iter()
        .filter(|e| matches!(e, ClaudeEvent::AwaitingPrompt))
        .count();
    assert_eq!(
        prompt_count, 1,
        "should only see the new end_turn row, got {events:?}"
    );
}

#[test]
fn partial_line_carries_across_ticks() {
    let dir = TempDir::new().unwrap();
    let (_mgr, path, rx) = make_adapter(&dir);

    // Write the line in two halves with a delay between to force
    // the watcher to fire twice. The adapter must not parse the
    // half-line and must concatenate before parsing. We scope
    // each write so the handle closes between them — macOS
    // FSEvents won't deliver modify events for an open file
    // until close, regardless of fsync.
    {
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(br#"{"type":"assistant","sessionId":"x","message":{"stop_reason":"#)
            .unwrap();
        f.sync_all().unwrap();
    }
    thread::sleep(Duration::from_millis(150));
    {
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"\"end_turn\",\"content\":[]}}\n").unwrap();
        f.sync_all().unwrap();
    }

    let events = drain(&rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
        "expected one AwaitingPrompt after concatenation, got {events:?}"
    );
    // And exactly one — not one per half-line.
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, ClaudeEvent::AwaitingPrompt))
            .count(),
        1
    );
}

#[test]
fn malformed_json_line_skipped_others_still_emit() {
    let dir = TempDir::new().unwrap();
    let (_mgr, path, rx) = make_adapter(&dir);

    let mut f = fs::File::create(&path).unwrap();
    writeln!(f, "not-json-at-all").unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();

    let events = drain(&rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
        "valid row after garbage should still emit, got {events:?}"
    );
}

#[test]
fn attach_before_file_exists_then_create() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");

    let (tx, rx) = mpsc::channel();
    let manager = test_manager();
    manager
        .attach_at(
            "h1",
            path.clone(),
            move |e| {
                tx.send(e).unwrap();
            },
            None,
        )
        .unwrap();

    // File doesn't exist yet — adapter should be waiting on the
    // parent-dir watcher. Create it now.
    thread::sleep(Duration::from_millis(50));
    let mut f = fs::File::create(&path).unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();

    let events = drain(&rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
        "expected AwaitingPrompt after late create, got {events:?}"
    );
}

#[test]
fn attach_to_pre_existing_session_ending_in_end_turn_starts_in_waiting() {
    // The resume bug: every Claude harness shows green on Skein
    // restart because the existing end_turn row was written
    // before we attached. Probe-on-attach fixes it by emitting
    // a synthetic AwaitingPrompt immediately.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    {
        let mut f = fs::File::create(&path).unwrap();
        // Realistic shape: a user prompt, a few tool_use rounds,
        // ending with an end_turn.
        writeln!(
            f,
            r#"{{"type":"user","sessionId":"x","message":{{"content":[{{"type":"text","text":"hi"}}]}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[{{"type":"tool_use","name":"Read","id":"t1","input":{{}}}}]}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"user","sessionId":"x","toolUseResult":"ok","message":{{"content":[]}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[{{"type":"text","text":"done"}}]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();
    }

    let (tx, rx) = mpsc::channel();
    let manager = test_manager();
    manager
        .attach_at("h1", path, move |e| tx.send(e).unwrap(), None)
        .unwrap();

    let events = drain_brief(&rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
        "history-probe should fire AwaitingPrompt synthetically, got {events:?}"
    );
}

#[test]
fn attach_to_pre_existing_session_mid_turn_starts_in_running() {
    // Mid-turn resume: last assistant row had tool_use, so the
    // session was interrupted while a tool was being called.
    // claude --resume picks up, but we shouldn't claim it's
    // awaiting input — it's still running.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    {
        let mut f = fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();
    }

    let (tx, rx) = mpsc::channel();
    let manager = test_manager();
    manager
        .attach_at("h1", path, move |e| tx.send(e).unwrap(), None)
        .unwrap();

    let events = drain_brief(&rx);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
        "mid-turn session must not start in waiting, got {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::AssistantTurn)),
        "mid-turn session should emit AssistantTurn to signal running, got {events:?}"
    );
}

/// #362: `TailState::first_read_logged` flips exactly once, on the
/// first tick that actually reads bytes off the main transcript —
/// not on an empty-file tick, and not again on a later tick once
/// it's already true. Drives `tick` directly against a hand-built
/// `TailState` rather than through `attach_at`'s watcher, since
/// what's under test is the flag flip, not the debouncer plumbing;
/// no tracing-subscriber harness needed because the assertion is
/// on the state field the log line guards, not on emitted output.
#[test]
fn first_read_logged_flips_once_per_attach() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    fs::write(&path, "").unwrap();
    let state = Arc::new(Mutex::new(TailState {
        harness_id: "h".into(),
        path: path.clone(),
        last_pos: 0,
        partial: String::new(),
        attached: true,
        ever_attached: true,
        fingerprint: Vec::new(),
        in_assistant_turn: false,
        local_command: LocalCommandTracker::default(),
        actions: None,
        subagents_dir: None,
        subagents: HashMap::new(),
        background: BackgroundState::default(),
        subagents_dir_read_failure_logged: false,
        first_read_logged: false,
        events_sent: 0,
        send_errors: 0,
        last_heartbeat: None,
        utf8_stall_at: None,
        utf8_stall_count: 0,
        utf8_stall_warned: false,
        last_cli_version: None,
    }));

    // Nothing written yet — a tick that reads zero bytes must not
    // flip the flag.
    tick(&state, &|_| {});
    assert!(!state.lock().first_read_logged);

    // Append a row — this tick's read is the first non-empty one.
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(f, r#"{{"type":"user"}}"#).unwrap();
    f.sync_all().unwrap();

    tick(&state, &|_| {});
    assert!(state.lock().first_read_logged);

    // A later empty tick must leave it set, not toggle it back.
    tick(&state, &|_| {});
    assert!(state.lock().first_read_logged);
}

/// #362: `events_sent` must count only events actually handed to
/// `on_event`, not everything a tick queued — a panic partway
/// through the dispatch loop must not credit events that were
/// never delivered. Two rows queue two events; the callback panics
/// on the second call, so only the first should be counted, and
/// the panic itself must still land in `send_errors`.
#[test]
fn events_sent_counts_only_events_dispatched_before_a_panic() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    fs::write(
        &path,
        concat!(
            r#"{"type":"assistant","message":{"stop_reason":"end_turn"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"stop_reason":"end_turn"}}"#,
            "\n",
        ),
    )
    .unwrap();
    let state = Arc::new(Mutex::new(TailState {
        harness_id: "h".into(),
        path: path.clone(),
        last_pos: 0,
        partial: String::new(),
        attached: true,
        ever_attached: true,
        fingerprint: Vec::new(),
        in_assistant_turn: false,
        local_command: LocalCommandTracker::default(),
        actions: None,
        subagents_dir: None,
        subagents: HashMap::new(),
        background: BackgroundState::default(),
        subagents_dir_read_failure_logged: false,
        first_read_logged: false,
        events_sent: 0,
        send_errors: 0,
        last_heartbeat: None,
        utf8_stall_at: None,
        utf8_stall_count: 0,
        utf8_stall_warned: false,
        last_cli_version: None,
    }));

    let calls = std::sync::atomic::AtomicUsize::new(0);
    tick(&state, &|_event| {
        let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        assert!(n != 1, "boom");
    });

    let s = state.lock();
    assert_eq!(
        s.events_sent, 1,
        "only the event dispatched before the panic should be counted, got {}",
        s.events_sent
    );
    assert_eq!(s.send_errors, 1);
}

#[test]
fn should_heartbeat_rate_limits_to_once_per_interval() {
    let now = Instant::now();
    assert!(
        should_heartbeat(None, now),
        "the very first heartbeat should fire immediately"
    );
    assert!(
        !should_heartbeat(Some(now), now),
        "must not fire again at the same instant"
    );
    let almost_due = now + Duration::from_secs(59);
    assert!(
        !should_heartbeat(Some(now), almost_due),
        "must not fire before the interval elapses"
    );
    let due = now + HEARTBEAT_INTERVAL;
    assert!(
        should_heartbeat(Some(now), due),
        "must fire once the interval has elapsed"
    );
}

#[test]
fn should_warn_utf8_stall_fires_once_at_threshold() {
    assert!(
        !should_warn_utf8_stall(UTF8_STALL_WARN_THRESHOLD - 1, false),
        "must not fire before the threshold"
    );
    assert!(
        should_warn_utf8_stall(UTF8_STALL_WARN_THRESHOLD, false),
        "must fire once the threshold is reached"
    );
    assert!(
        !should_warn_utf8_stall(UTF8_STALL_WARN_THRESHOLD, true),
        "must not re-fire once already warned for this run"
    );
    assert!(
        should_warn_utf8_stall(UTF8_STALL_WARN_THRESHOLD + 5, false),
        "must fire past the threshold too, as long as it hasn't warned yet"
    );
}

/// Review fix for #410: `DirId::of` must never fail for a path that
/// exists — a directory whose identity genuinely can't be read (an
/// unsupported filesystem, or a metadata race) degrades to an
/// "unknown" identity rather than dropping out of the watch set
/// entirely. Two "unknown" identities for the SAME still-existing
/// path must compare equal — otherwise `rearm`'s `desired == armed`
/// fast path would spuriously call every single such directory
/// "changed" on every pass, forever.
#[test]
fn dir_id_of_an_unreadable_path_is_an_unknown_identity_equal_to_itself() {
    let dir = TempDir::new().unwrap();
    // A path with no metadata to read — the simplest cross-platform
    // stand-in for "identity unavailable" (the real-world case is a
    // filesystem where `created()` errors on an otherwise perfectly
    // normal, existing directory).
    let unreadable = dir.path().join("does-not-exist");
    let a = DirId::of(&unreadable);
    let b = DirId::of(&unreadable);
    assert_eq!(
        a, b,
        "two 'unknown' identities for the same path must compare equal"
    );
}

/// End-to-end (through `tick`, not just the pure decision fn): a
/// main transcript stuck on the same invalid byte at the same
/// `last_pos` climbs the stall counter one per tick, warns exactly
/// once at the threshold, and a later successful read clears all
/// three fields — the actual field-mutating logic `tick` uses.
#[test]
fn utf8_stall_counter_tracks_consecutive_failures_and_resets_on_recovery() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    // A lone continuation byte: not a prefix of anything, so it
    // never becomes valid UTF-8 no matter how many times it's
    // re-read — the stall persists until the file itself changes.
    fs::write(&path, [0x80]).unwrap();
    let state = Arc::new(Mutex::new(TailState {
        harness_id: "h".into(),
        path: path.clone(),
        last_pos: 0,
        partial: String::new(),
        attached: true,
        ever_attached: true,
        fingerprint: Vec::new(),
        in_assistant_turn: false,
        local_command: LocalCommandTracker::default(),
        actions: None,
        subagents_dir: None,
        subagents: HashMap::new(),
        background: BackgroundState::default(),
        subagents_dir_read_failure_logged: false,
        first_read_logged: false,
        events_sent: 0,
        send_errors: 0,
        last_heartbeat: None,
        utf8_stall_at: None,
        utf8_stall_count: 0,
        utf8_stall_warned: false,
        last_cli_version: None,
    }));

    for expected in 1..UTF8_STALL_WARN_THRESHOLD {
        tick(&state, &|_| {});
        let s = state.lock();
        assert_eq!(
            s.utf8_stall_count, expected,
            "stall count should climb by exactly one per tick"
        );
        assert_eq!(s.utf8_stall_at, Some(0));
        assert!(!s.utf8_stall_warned, "must not warn before the threshold");
    }

    tick(&state, &|_| {});
    {
        let s = state.lock();
        assert_eq!(s.utf8_stall_count, UTF8_STALL_WARN_THRESHOLD);
        assert!(
            s.utf8_stall_warned,
            "must warn once the threshold is reached"
        );
    }

    // Recovery: the file changes underneath the stall — the next
    // tick decodes cleanly and must rearm every field.
    fs::write(&path, "{\"type\":\"user\"}\n").unwrap();
    tick(&state, &|_| {});
    let s = state.lock();
    assert_eq!(
        s.utf8_stall_count, 0,
        "a successful read must clear the stall count"
    );
    assert_eq!(s.utf8_stall_at, None);
    assert!(
        !s.utf8_stall_warned,
        "a successful read must rearm the warn guard"
    );
}
