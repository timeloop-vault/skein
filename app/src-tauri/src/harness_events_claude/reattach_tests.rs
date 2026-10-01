//! The supervisor: dead-tail detection, backoff and manual and automatic reattach.

use super::subagent_tests::make_adapter_with_existing_main;
use super::tests::{drain, drain_brief, make_adapter};
use super::*;
use std::fs;
use std::io::Write;
use std::sync::mpsc;
use tempfile::TempDir;

/// A pass over an adapter whose watched directories haven't
/// changed at all since the last one must not touch the watcher —
/// `rearm`'s fast-path equality check is what keeps a healthy
/// harness's supervisor pass a no-op.
#[test]
fn supervise_once_does_not_rearm_a_healthy_unchanged_adapter() {
    let dir = TempDir::new().unwrap();
    let (mgr, _path, rx) = make_adapter(&dir);
    drain_brief(&rx);

    let rearmed = mgr.supervise_once();
    assert_eq!(
        rearmed, 0,
        "a healthy, unchanged adapter should not be re-armed"
    );
}

// ── #410: dead-tail detection and reattach ─────────────────────

/// The headline #410 scenario: a watch dies in a way `rearm` can
/// never notice (nothing about the directory's identity changed),
/// the transcript keeps growing, and two supervisor passes across
/// the `DEAD_TAIL_AFTER` window are what it takes to confirm and
/// recover it — the first pass only starts tracking the stall, the
/// second (once it's old enough) reattaches. Exactly one
/// `AwaitingPrompt` proves the reattach happened exactly once, not
/// zero or twice.
#[test]
fn dead_tail_is_reattached_and_settles_from_transcript() {
    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    drain(&rx);

    mgr.simulate_dead_watch_for_test("harness-1");

    // Grows the file while nothing is watching — the symptom.
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[{{"type":"text","text":"done"}}]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();

    // Pass 1: notices the growth, starts tracking the stall — not
    // dead yet, and rearm has nothing to fix (the directory itself
    // never changed identity).
    let rearmed = mgr.supervise_once();
    assert_eq!(rearmed, 0, "the directory's identity never changed");
    assert!(
        drain_brief(&rx).is_empty(),
        "must not reattach on the very first observation of growth"
    );

    mgr.backdate_stall_for_test("harness-1");

    // Pass 2: same last_pos, now old enough — dead, reattach.
    mgr.supervise_once();

    let events = drain(&rx);
    let awaiting_count = events
        .iter()
        .filter(|e| matches!(e, ClaudeEvent::AwaitingPrompt))
        .count();
    assert_eq!(
        awaiting_count, 1,
        "expected exactly one reattach's worth of AwaitingPrompt, got {events:?}"
    );
}

/// A tail that keeps up with the file it's watching must never be
/// reattached, across several passes — the acceptance criterion is
/// literally "no churn" for the common, healthy case.
#[test]
fn healthy_tail_is_never_reattached() {
    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    drain(&rx);

    for i in 0..3 {
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[{{"type":"tool_use","name":"Tool{i}"}}]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();
        // Close before draining: macOS FSEvents reports the change
        // on close, not per write (#437; see
        // `main_events_keep_flowing_after_sidecars_appear_post_attach`).
        drop(f);
        // Let the (still perfectly healthy) live watch catch up
        // normally before checking — `drain` waits for it.
        let events = drain(&rx);
        assert!(
            !events.is_empty(),
            "iteration {i}: the live watch should still be delivering normally"
        );
        let rearmed = mgr.supervise_once();
        assert_eq!(rearmed, 0, "iteration {i}: nothing about the watch changed");
    }

    assert!(
        drain_brief(&rx).is_empty(),
        "a healthy tail must never trigger a spurious reattach"
    );
}

/// At most one AUTOMATIC re-attach per `REATTACH_BACKOFF`: a second
/// dead-tail cycle that starts (and gets confirmed) well within the
/// backoff window of the first must not reattach again, even though
/// `check_dead_tail` genuinely reports it as dead.
#[test]
fn auto_reattach_respects_backoff() {
    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    drain(&rx);

    mgr.simulate_dead_watch_for_test("harness-1");
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();
    mgr.supervise_once();
    mgr.backdate_stall_for_test("harness-1");
    mgr.supervise_once();
    let first = drain(&rx);
    assert!(
        !first.is_empty(),
        "expected the first automatic reattach to produce events"
    );

    // Break the freshly re-attached adapter's watch again right
    // away and grow the file again — a second dead cycle, well
    // within `REATTACH_BACKOFF` of the first.
    mgr.simulate_dead_watch_for_test("harness-1");
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[{{"type":"tool_use","name":"X"}}]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();
    mgr.supervise_once();
    mgr.backdate_stall_for_test("harness-1");
    mgr.supervise_once();

    assert!(
        drain_brief(&rx).is_empty(),
        "a second automatic reattach within REATTACH_BACKOFF must not happen"
    );
}

/// `ClaudeEventsManager::reattach` — the manual, backoff-free path
/// behind the `claude_events_reattach` Tauri command: `NotAttached`
/// for an id with no adapter at all, `Healthy` when nothing needs
/// fixing, `Reattached` when the tail actually was dead — and,
/// unlike the automatic path, immediately, with no
/// `DEAD_TAIL_AFTER` wait and no backoff.
#[test]
fn manual_reattach_outcomes() {
    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    drain(&rx);

    assert_eq!(
        mgr.reattach("no-such-harness").unwrap(),
        ReattachOutcome::NotAttached
    );

    assert_eq!(
        mgr.reattach("harness-1").unwrap(),
        ReattachOutcome::Healthy,
        "nothing changed since attach"
    );
    assert!(drain_brief(&rx).is_empty());

    mgr.simulate_dead_watch_for_test("harness-1");
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[{{"type":"text","text":"done"}}]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();

    assert_eq!(
        mgr.reattach("harness-1").unwrap(),
        ReattachOutcome::Reattached,
        "manual reattach must recover a dead tail immediately, no backoff or wait"
    );
    let events = drain(&rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
        "expected the manual reattach to re-read the transcript, got {events:?}"
    );
}

/// Review fix for #410: `supervise_map` collects `ReattachJob`s
/// under the registry lock, then installs each one — separately,
/// after releasing it. A `detach` landing in that gap must not let
/// the stale reattach resurrect the (now closed) harness. Runs the
/// two phases as an explicit test-only seam rather than racing real
/// threads — see `collect_dead_tail_jobs_for_test`'s doc comment.
#[test]
fn reattach_abandoned_when_detached_between_collect_and_perform() {
    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    drain(&rx);

    mgr.simulate_dead_watch_for_test("harness-1");
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();
    mgr.supervise_once();
    mgr.backdate_stall_for_test("harness-1");

    let jobs = mgr.collect_dead_tail_jobs_for_test();
    assert_eq!(jobs.len(), 1, "expected exactly one dead-tail job");

    // The race: the harness is closed between collection and
    // install.
    assert!(mgr.detach("harness-1"));

    mgr.perform_reattach_jobs_for_test(jobs);

    assert_eq!(
        mgr.reattach("harness-1").unwrap(),
        ReattachOutcome::NotAttached,
        "the abandoned reattach must not have resurrected a detached harness"
    );
    // The abandoned reattach's own construction (re-reading the
    // transcript's existing history) may have already emitted a
    // one-time synthetic event through the OLD channel before the
    // CAS ever ran — harmless, since nothing installed it. What
    // must NOT happen is a leaked watcher: drain whatever
    // construction-time backlog there was, then confirm nothing
    // MORE arrives after it, proving the abandoned adapter's
    // debouncer was actually dropped rather than left running.
    drain_brief(&rx);
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[{{"type":"tool_use","name":"X"}}]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();
    // Close, and wait the full `drain` window: macOS FSEvents only
    // reports the change on close and can take longer than
    // `drain_brief`'s 300 ms, so either shortcut would make this
    // absence check pass vacuously there (#437).
    drop(f);
    assert!(
        drain(&rx).is_empty(),
        "a detached harness must not keep receiving events from a leaked, abandoned watcher"
    );
}

/// Same race, the other direction: a fresh caller-driven `attach()`
/// (a respawn — a new session on the same harness id) lands between
/// collection and install. The fresh adapter must survive; the
/// stale reattach's own (already-built) adapter is simply dropped.
#[test]
fn reattach_abandoned_when_fresh_attach_lands_between_collect_and_perform() {
    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    drain(&rx);

    mgr.simulate_dead_watch_for_test("harness-1");
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();
    mgr.supervise_once();
    mgr.backdate_stall_for_test("harness-1");

    let jobs = mgr.collect_dead_tail_jobs_for_test();
    assert_eq!(jobs.len(), 1, "expected exactly one dead-tail job");

    // The race: a fresh attach (respawn) lands on the same harness
    // id before the stale reattach installs.
    let (tx2, rx2) = mpsc::channel();
    let fresh_path = dir.path().join("fresh-session.jsonl");
    fs::write(&fresh_path, "").unwrap();
    mgr.attach_at(
        "harness-1",
        fresh_path.clone(),
        move |e| {
            let _ = tx2.send(e);
        },
        None,
    )
    .unwrap();

    mgr.perform_reattach_jobs_for_test(jobs);

    // Confirm the FRESH adapter is the one that survived: it must
    // still be tailing `fresh_path`, not the stale `path`.
    let mut f2 = fs::OpenOptions::new()
        .append(true)
        .open(&fresh_path)
        .unwrap();
    writeln!(
        f2,
        r#"{{"type":"assistant","sessionId":"y","message":{{"stop_reason":"end_turn","content":[]}}}}"#
    )
    .unwrap();
    f2.sync_all().unwrap();
    // Close before draining: macOS FSEvents reports the change on
    // close, not per write (#437).
    drop(f2);
    let events = drain(&rx2);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
        "the fresh adapter must have survived the abandoned reattach, got {events:?}"
    );
}
