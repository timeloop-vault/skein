//! #497: a subagent whose sidecar is missing or half-written at first
//! sighting gets its label once the sidecar is readable.

use super::subagent_tests::make_adapter_with_existing_main;
use super::tests::{append_block, await_bootstrap, drain_until};
use super::*;
use std::fs;
use std::io::Write;
use tempfile::TempDir;

const END_TURN: &str = r#"{"type":"assistant","isSidechain":true,"agentId":"a1","message":{"stop_reason":"end_turn","content":[]}}"#;

fn label_events(events: &[ClaudeEvent]) -> Vec<&ClaudeEvent> {
    events
        .iter()
        .filter(|e| matches!(e, ClaudeEvent::SubagentLabel { .. }))
        .collect()
}

fn started_unlabelled(evs: &[ClaudeEvent]) -> bool {
    evs.iter()
        .any(|e| matches!(e, ClaudeEvent::SubagentStart { .. }))
}

fn label_seen(evs: &[ClaudeEvent]) -> bool {
    !label_events(evs).is_empty()
}

/// Poke the main transcript and wait for this poke's own
/// `ToolUseStart`: a tick that produced it also scanned the sidecars,
/// so what it did not emit is absent, not late.
fn poke_and_settle(
    path: &std::path::Path,
    rx: &std::sync::mpsc::Receiver<ClaudeEvent>,
) -> Vec<ClaudeEvent> {
    let tool = poke_main(path);
    drain_until(rx, |evs| {
        evs.iter()
            .any(|e| matches!(e, ClaudeEvent::ToolUseStart { name } if *name == tool))
    })
}

/// Touch the main transcript so the shared debouncer ticks. Writes two
/// tool-use rows in one block: the first may open the turn
/// (`AssistantTurn`), so the second, naming a tool unique to this poke,
/// is the one guaranteed to yield a `ToolUseStart`. Returns that name.
fn poke_main(path: &std::path::Path) -> String {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let tool = format!(
        "Poke{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let row = |name: &str| {
        format!(
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[{{"type":"tool_use","name":"{name}"}}]}}}}"#
        )
    };
    append_block(path, &[&row("PokeOpen"), &row(&tool)]);
    tool
}

#[test]
fn late_sidecar_labels_subagent_once_and_end_carries_it() {
    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    await_bootstrap(&rx);

    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    mgr.supervise_once();
    let sub_path = sub_dir.join("agent-a1.jsonl");
    fs::write(
        &sub_path,
        r#"{"type":"assistant","isSidechain":true,"agentId":"a1","message":{"stop_reason":"tool_use","content":[]}}"#
            .to_owned()
            + "\n",
    )
    .unwrap();
    let events = drain_until(&rx, started_unlabelled);
    assert!(
        events.iter().any(|e| matches!(
            e,
            ClaudeEvent::SubagentStart {
                agent_type: None,
                ..
            }
        )),
        "got {events:?}"
    );
    assert!(label_events(&events).is_empty(), "got {events:?}");

    // Half-written sidecar: still nothing.
    let meta_path = sub_dir.join("agent-a1.meta.json");
    fs::write(&meta_path, r#"{"agentType":"expl"#).unwrap();
    assert!(label_events(&poke_and_settle(&path, &rx)).is_empty());

    // Now valid: one label, on a tick where the transcript is quiet.
    fs::write(
        &meta_path,
        r#"{"agentType":"explore","description":"Map it"}"#,
    )
    .unwrap();
    poke_main(&path);
    let events = drain_until(&rx, label_seen);
    assert!(
        events.iter().any(|e| matches!(
            e,
            ClaudeEvent::SubagentLabel { agent_id, agent_type, description }
                if agent_id == "a1"
                    && agent_type.as_deref() == Some("explore")
                    && description.as_deref() == Some("Map it")
        )),
        "got {events:?}"
    );

    // Both known: no further label on later ticks.
    assert!(label_events(&poke_and_settle(&path, &rx)).is_empty());

    // The end event carries the late label.
    let mut f = fs::OpenOptions::new().append(true).open(&sub_path).unwrap();
    writeln!(f, "{END_TURN}").unwrap();
    f.sync_all().unwrap();
    // Close before draining: macOS FSEvents reports the change on
    // close, not per write (#437, #666).
    drop(f);
    let events = drain_until(&rx, |evs| {
        evs.iter()
            .any(|e| matches!(e, ClaudeEvent::SubagentEnd { .. }))
    });
    assert!(
        events.iter().any(|e| matches!(
            e,
            ClaudeEvent::SubagentEnd { agent_type, .. }
                if agent_type.as_deref() == Some("explore")
        )),
        "got {events:?}"
    );
    assert!(label_events(&events).is_empty());
}

/// A sidecar that parses is final even when it lacks a field: it is
/// not read again, so a later rewrite adding `description` is ignored.
#[test]
fn parsed_sidecar_missing_a_field_is_not_reread() {
    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    await_bootstrap(&rx);

    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    mgr.supervise_once();
    fs::write(
        sub_dir.join("agent-a1.jsonl"),
        r#"{"type":"assistant","isSidechain":true,"agentId":"a1","message":{"stop_reason":"tool_use","content":[]}}"#
            .to_owned()
            + "\n",
    )
    .unwrap();
    drain_until(&rx, started_unlabelled);

    let meta_path = sub_dir.join("agent-a1.meta.json");
    fs::write(&meta_path, r#"{"agentType":"explore"}"#).unwrap();
    poke_main(&path);
    let events = drain_until(&rx, label_seen);
    assert!(
        events.iter().any(|e| matches!(
            e,
            ClaudeEvent::SubagentLabel { agent_type, description, .. }
                if agent_type.as_deref() == Some("explore") && description.is_none()
        )),
        "got {events:?}"
    );

    fs::write(
        &meta_path,
        r#"{"agentType":"explore","description":"late"}"#,
    )
    .unwrap();
    assert!(label_events(&poke_and_settle(&path, &rx)).is_empty());
}

/// A finished subagent whose sidecar never appeared stops being read:
/// a sidecar written afterwards is not picked up.
#[test]
fn finished_subagent_is_not_reread_for_its_label() {
    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    await_bootstrap(&rx);

    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    mgr.supervise_once();
    let sub_path = sub_dir.join("agent-a1.jsonl");
    fs::write(
        &sub_path,
        r#"{"type":"assistant","isSidechain":true,"agentId":"a1","message":{"stop_reason":"tool_use","content":[]}}"#
            .to_owned()
            + "
",
    )
    .unwrap();
    drain_until(&rx, started_unlabelled);
    let mut f = fs::OpenOptions::new().append(true).open(&sub_path).unwrap();
    writeln!(f, "{END_TURN}").unwrap();
    f.sync_all().unwrap();
    // Close before draining: macOS FSEvents reports the change on
    // close, not per write (#437, #666).
    drop(f);
    let events = drain_until(&rx, |evs| {
        evs.iter()
            .any(|e| matches!(e, ClaudeEvent::SubagentEnd { .. }))
    });
    assert!(
        events.iter().any(|e| matches!(
            e,
            ClaudeEvent::SubagentEnd {
                agent_type: None,
                ..
            }
        )),
        "got {events:?}"
    );

    fs::write(
        sub_dir.join("agent-a1.meta.json"),
        r#"{"agentType":"explore","description":"late"}"#,
    )
    .unwrap();
    assert!(label_events(&poke_and_settle(&path, &rx)).is_empty());
}
