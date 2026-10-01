//! Concurrent attaches and sidecar churn against one manager.

use super::adapter::{ActionPersistence, AttachInfo};
use super::subagent_tests::make_adapter_with_existing_main;
use super::tests::drain;
use super::*;
use crate::harness_actions_claude::ActionExtractor;
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Barrier;
use std::sync::mpsc;
use std::time::Duration;
use std::{fs, thread};
use tempfile::TempDir;

// ── concurrent attach across many harnesses (#362) ──────────────
//
// Production shape: 5 Claude harnesses spawned within ~4 s, each
// in a brand-new worktree, so each project dir under
// `~/.claude/projects` did not exist yet. `claude_events_attach`
// (lib.rs) runs every `attach` call on tokio's `spawn_blocking`
// pool against ONE shared `ClaudeEventsManager` — i.e. concurrently,
// on different OS threads. 4 of 5 adapters never read their
// transcript: zero rows persisted, no events, ever. These tests
// reproduce that shape directly against `attach_at` with plain
// `std::thread`s released together by a `Barrier`, skipping tokio
// entirely; one manager and one on-disk `Database` shared across
// all harnesses, action persistence ON (production always runs
// with it on, #80).

/// One harness's fixture for a concurrent-attach run.
struct ConcurrentHarness {
    harness_id: String,
    room_id: String,
    path: PathBuf,
    rx: mpsc::Receiver<ClaudeEvent>,
}

/// Attach `N` harnesses to `N` distinct transcript paths — each
/// under its own never-before-seen project dir, mirroring one dir
/// per brand-new worktree — releasing all `N` `attach_at` calls
/// together via a `Barrier` so they race on parent-dir creation and
/// `notify` watcher setup the way five simultaneous worktree spawns
/// did in production. Then, from `N` more threads released the same
/// way, mimics Claude actually writing each transcript: a user
/// prompt row, a tool call + its result, and a terminal `end_turn`
/// row, each write separated by a short sleep so the watcher has to
/// fire more than once per harness — same as a real turn, and the
/// path most likely to race a debounced tick against a fresh
/// `attach_at` still setting up its `TailState`.
///
/// `precreate_parent` / `precreate_file` select which of #362's
/// real timings this run reproduces:
///   - neither: the project dir doesn't exist yet at attach time —
///     the production incident itself
///   - `precreate_parent` only: the dir exists (an earlier session
///     wrote there already) but this session's `.jsonl` doesn't
///   - both: the file already has a row in it before `attach_at`
///     runs — Claude won the race and wrote first
///
/// Returns one `bool` per harness: whether it observed its
/// `AwaitingPrompt` end-of-turn within a single 5 s budget shared
/// across all `N` (not 5 s each serially, which would let a
/// handful of stuck harnesses blow the test out to `N * 5` s).
fn run_concurrent_attach_variant(precreate_parent: bool, precreate_file: bool) -> Vec<bool> {
    const N: usize = 6;
    let root = TempDir::new().unwrap();
    let db = Arc::new(crate::db::Database::open(&root.path().join("t.db")).unwrap());
    let manager = Arc::new(ClaudeEventsManager::new_for_test(Arc::clone(&db)));

    let mut senders = Vec::with_capacity(N);
    let mut harnesses = Vec::with_capacity(N);
    for i in 0..N {
        let harness_id = format!("h{i}");
        let room_id = format!("r{i}");
        // A distinct, never-before-seen project dir per harness.
        let parent = root.path().join("projects").join(format!("proj-{i}"));
        let path = parent.join(format!("{}.jsonl", uuid::Uuid::new_v4()));
        if precreate_parent || precreate_file {
            fs::create_dir_all(&parent).unwrap();
        }
        if precreate_file {
            let mut f = fs::File::create(&path).unwrap();
            writeln!(
                f,
                r#"{{"type":"user","sessionId":"s{i}","message":{{"content":[{{"type":"text","text":"hi"}}]}}}}"#
            )
            .unwrap();
            f.sync_all().unwrap();
        }
        let (tx, rx) = mpsc::channel();
        senders.push((harness_id.clone(), room_id.clone(), path.clone(), tx));
        harnesses.push(ConcurrentHarness {
            harness_id,
            room_id,
            path,
            rx,
        });
    }

    // Attach all N together, released by a barrier — the race is
    // inside `attach_at` (parent-dir creation, `notify` watcher
    // setup), not in anything downstream.
    let attach_barrier = Arc::new(Barrier::new(N));
    let attach_results: Vec<Result<AttachInfo, ClaudeEventsError>> = thread::scope(|scope| {
        let handles: Vec<_> = senders
            .into_iter()
            .map(|(harness_id, room_id, path, tx)| {
                let manager = Arc::clone(&manager);
                let db = Arc::clone(&db);
                let barrier = Arc::clone(&attach_barrier);
                scope.spawn(move || {
                    let persistence = Some(ActionPersistence {
                        extractor: ActionExtractor::new(),
                        db,
                        harness_id: harness_id.clone(),
                        room_id,
                        cwd: String::new(),
                        app: None,
                    });
                    barrier.wait();
                    manager.attach_at(
                        &harness_id,
                        path,
                        move |e| {
                            let _ = tx.send(e);
                        },
                        persistence,
                    )
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    for (i, result) in attach_results.iter().enumerate() {
        assert!(
            result.is_ok(),
            "harness {i} failed to attach: {:?}",
            result.as_ref().err().map(ToString::to_string)
        );
    }

    // Now mimic Claude actually writing the transcript.
    let write_barrier = Arc::new(Barrier::new(N));
    thread::scope(|scope| {
        for (i, h) in harnesses.iter().enumerate() {
            let path = h.path.clone();
            let barrier = Arc::clone(&write_barrier);
            scope.spawn(move || {
                barrier.wait();
                if !precreate_file {
                    // #362: `attach_at` no longer pre-creates the
                    // project dir (that was Skein inventing a
                    // directory Claude might never write to) — so
                    // the write side of this fixture has to do what
                    // Claude itself does, create its own project
                    // dir lazily on first write, or this `File::
                    // create` fails outright when `precreate_parent`
                    // is also false.
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    let mut f = fs::File::create(&path).unwrap();
                    writeln!(
                        f,
                        r#"{{"type":"user","sessionId":"s{i}","message":{{"content":[{{"type":"text","text":"hi"}}]}}}}"#
                    )
                    .unwrap();
                    f.sync_all().unwrap();
                }
                thread::sleep(Duration::from_millis(120));
                {
                    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
                    writeln!(
                        f,
                        r#"{{"type":"assistant","uuid":"a{i}","timestamp":"2026-05-15T21:16:{:02}.000Z","message":{{"content":[{{"type":"tool_use","id":"toolu_{i}","name":"Bash","input":{{"command":"echo hi"}}}}]}}}}"#,
                        20 + i
                    )
                    .unwrap();
                    f.sync_all().unwrap();
                }
                thread::sleep(Duration::from_millis(120));
                {
                    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
                    writeln!(
                        f,
                        r#"{{"type":"user","timestamp":"2026-05-15T21:16:{:02}.000Z","toolUseResult":{{"stdout":"hi"}},"message":{{"content":[{{"type":"tool_result","tool_use_id":"toolu_{i}","content":"hi","is_error":false}}]}}}}"#,
                        40 + i
                    )
                    .unwrap();
                    f.sync_all().unwrap();
                }
                thread::sleep(Duration::from_millis(120));
                {
                    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
                    writeln!(
                        f,
                        r#"{{"type":"assistant","sessionId":"s{i}","message":{{"stop_reason":"end_turn","content":[{{"type":"text","text":"done"}}]}}}}"#
                    )
                    .unwrap();
                    f.sync_all().unwrap();
                }
            });
        }
    });

    // Poll every harness's receiver in round-robin against ONE
    // shared 5 s deadline. Also drives `supervise_once` every
    // iteration (#362): with fresh parent dirs, the watch armed at
    // attach time is the nearest existing ancestor, not the real
    // project dir — production's background thread is what
    // notices the real dir appear and re-arms onto it (see
    // `Adapter::rearm`); `new_for_test` spawns no such thread, so
    // this loop stands in for it, at a much tighter interval than
    // `SUPERVISE_INTERVAL` so the fixture doesn't need anywhere
    // near this test's 5 s budget to converge.
    let mut satisfied = vec![false; N];
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline && satisfied.iter().any(|s| !s) {
        manager.supervise_once();
        for (i, h) in harnesses.iter().enumerate() {
            if satisfied[i] {
                continue;
            }
            while let Ok(event) = h.rx.try_recv() {
                if matches!(event, ClaudeEvent::AwaitingPrompt) {
                    satisfied[i] = true;
                }
            }
        }
        thread::sleep(Duration::from_millis(20));
    }

    // Action persistence (#80): every harness that reached
    // AwaitingPrompt must also have its tool-call row recorded.
    for (i, h) in harnesses.iter().enumerate() {
        if !satisfied[i] {
            continue;
        }
        let rows = db
            .recent_harness_actions_by_room(&h.room_id, -1, 100)
            .unwrap();
        assert!(
            rows.iter().any(|a| a.harness_id == h.harness_id),
            "harness {i} reached AwaitingPrompt but persisted no action row"
        );
    }

    satisfied
}

#[test]
fn concurrent_attach_fresh_parent_dirs_all_succeed() {
    let satisfied = run_concurrent_attach_variant(false, false);
    assert!(
        satisfied.iter().all(|s| *s),
        "not every harness completed its turn (index -> ok): {:?}",
        satisfied.iter().enumerate().collect::<Vec<_>>()
    );
}

#[test]
fn concurrent_attach_existing_parent_missing_file_all_succeed() {
    let satisfied = run_concurrent_attach_variant(true, false);
    assert!(
        satisfied.iter().all(|s| *s),
        "not every harness completed its turn (index -> ok): {:?}",
        satisfied.iter().enumerate().collect::<Vec<_>>()
    );
}

#[test]
fn concurrent_attach_preexisting_file_all_succeed() {
    let satisfied = run_concurrent_attach_variant(true, true);
    assert!(
        satisfied.iter().all(|s| *s),
        "not every harness completed its turn (index -> ok): {:?}",
        satisfied.iter().enumerate().collect::<Vec<_>>()
    );
}

// ── #362: heartbeat, sidecar discovery, utf8 stall ─────────────

/// The headline #362 scenario: a subagent sidecar appears after
/// attach, and the main transcript's own events must keep flowing
/// exactly as before — a sidecar joining the tailed set must never
/// starve `tick`'s main-file half.
#[test]
fn main_events_keep_flowing_after_sidecars_appear_post_attach() {
    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    drain(&rx);

    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    // #362: the dir didn't exist at attach, so a supervisor pass is
    // what notices it and arms the watch — production's background
    // thread does this on its own timer; `supervise_once` stands in
    // for it here.
    mgr.supervise_once();
    fs::write(
        sub_dir.join("agent-s1.meta.json"),
        r#"{"agentType":"explore","description":"Look around"}"#,
    )
    .unwrap();
    let mut sf = fs::File::create(sub_dir.join("agent-s1.jsonl")).unwrap();
    writeln!(
        sf,
        r#"{{"type":"assistant","isSidechain":true,"agentId":"s1","message":{{"stop_reason":"tool_use","content":[]}}}}"#
    )
    .unwrap();
    sf.sync_all().unwrap();
    // Every handle is closed before its drain. macOS FSEvents reports
    // a content change when the writer *closes* the file, not per
    // write, so an append through a still-open handle is invisible
    // there until it drops — Linux and Windows report each write.
    // Claude Code itself closes after every append (no running
    // `claude` holds its transcript open), so closing is also what
    // mirrors production.
    drop(sf);

    let events = drain(&rx);
    assert!(
        events.iter().any(|e| matches!(
            e,
            ClaudeEvent::SubagentStart { agent_id, initial, .. }
                if agent_id == "s1" && !initial
        )),
        "expected a live SubagentStart for s1, got {events:?}"
    );

    // The main transcript must keep producing its own events with
    // a subagent now mid-flight.
    let mut mf = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
        mf,
        r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[{{"type":"tool_use","name":"Read"}}]}}}}"#
    )
    .unwrap();
    mf.sync_all().unwrap();
    drop(mf);

    let events = drain(&rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::AssistantTurn)),
        "expected the main transcript's tool-use row to still fire AssistantTurn, got {events:?}"
    );

    let mut mf = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
        mf,
        r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
    )
    .unwrap();
    mf.sync_all().unwrap();
    drop(mf);

    let events = drain(&rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
        "expected the main transcript's end_turn row to still fire AwaitingPrompt, got {events:?}"
    );

    let mut sf = fs::OpenOptions::new()
        .append(true)
        .open(sub_dir.join("agent-s1.jsonl"))
        .unwrap();
    writeln!(
        sf,
        r#"{{"type":"assistant","isSidechain":true,"agentId":"s1","message":{{"stop_reason":"end_turn","content":[]}}}}"#
    )
    .unwrap();
    sf.sync_all().unwrap();
    drop(sf);

    let events = drain(&rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::SubagentEnd { agent_id, .. } if agent_id == "s1")),
        "expected SubagentEnd for s1, got {events:?}"
    );
}

/// Stress variant: five subagents churn (appear, run, finish) while
/// the main transcript writes its own turns, all under one attach.
/// Every main row must still produce its event and every subagent
/// must still get exactly one Start and one End — bounded by one
/// shared wall-clock deadline so a real regression (a stuck tail)
/// fails the test instead of hanging it.
#[test]
fn main_and_subagent_events_survive_concurrent_sidecar_churn() {
    const MAIN_TURNS: usize = 5;
    const SUBAGENTS: usize = 5;

    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    drain(&rx);

    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    // #362: arm the watch on the just-created dir before the churn
    // threads start writing into it.
    mgr.supervise_once();

    let barrier = Arc::new(Barrier::new(1 + SUBAGENTS));

    let main_path = path.clone();
    let main_barrier = Arc::clone(&barrier);
    let main_thread = thread::spawn(move || {
        main_barrier.wait();
        for i in 0..MAIN_TURNS {
            let mut f = fs::OpenOptions::new()
                .append(true)
                .open(&main_path)
                .unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[{{"type":"tool_use","name":"Tool{i}"}}]}}}}"#
            )
            .unwrap();
            f.sync_all().unwrap();
            thread::sleep(Duration::from_millis(15));
            let mut f = fs::OpenOptions::new()
                .append(true)
                .open(&main_path)
                .unwrap();
            writeln!(
                f,
                r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
            )
            .unwrap();
            f.sync_all().unwrap();
            thread::sleep(Duration::from_millis(15));
        }
    });

    let mut sub_threads = Vec::new();
    for i in 0..SUBAGENTS {
        let sub_dir = sub_dir.clone();
        let sub_barrier = Arc::clone(&barrier);
        sub_threads.push(thread::spawn(move || {
            sub_barrier.wait();
            thread::sleep(Duration::from_millis(5 * i as u64));
            let sub_path = sub_dir.join(format!("agent-s{i}.jsonl"));
            fs::write(
                &sub_path,
                format!(
                    "{{\"type\":\"assistant\",\"isSidechain\":true,\"agentId\":\"s{i}\",\"message\":{{\"stop_reason\":\"end_turn\",\"content\":[]}}}}\n"
                ),
            )
            .unwrap();
        }));
    }

    main_thread.join().unwrap();
    for h in sub_threads {
        h.join().unwrap();
    }

    let mut events = Vec::new();
    let mut assistant_turns = 0usize;
    let mut awaiting_prompts = 0usize;
    let mut started: HashSet<String> = HashSet::new();
    let mut ended: HashSet<String> = HashSet::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
        let Ok(ev) = rx.recv_timeout(remaining.min(Duration::from_millis(200))) else {
            continue;
        };
        match &ev {
            ClaudeEvent::AssistantTurn => assistant_turns += 1,
            ClaudeEvent::AwaitingPrompt => awaiting_prompts += 1,
            ClaudeEvent::SubagentStart { agent_id, .. } => {
                started.insert(agent_id.clone());
            }
            ClaudeEvent::SubagentEnd { agent_id, .. } => {
                ended.insert(agent_id.clone());
            }
            _ => {}
        }
        events.push(ev);
        if assistant_turns >= MAIN_TURNS
            && awaiting_prompts >= MAIN_TURNS
            && started.len() >= SUBAGENTS
            && ended.len() >= SUBAGENTS
        {
            break;
        }
    }

    assert_eq!(
        assistant_turns, MAIN_TURNS,
        "lost a main AssistantTurn under sidecar churn, got {events:?}"
    );
    assert_eq!(
        awaiting_prompts, MAIN_TURNS,
        "lost a main AwaitingPrompt under sidecar churn, got {events:?}"
    );
    assert_eq!(
        started.len(),
        SUBAGENTS,
        "lost a SubagentStart under sidecar churn, got {events:?}"
    );
    assert_eq!(
        ended.len(),
        SUBAGENTS,
        "lost a SubagentEnd under sidecar churn, got {events:?}"
    );
}
