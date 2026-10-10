//! The subagent sidecar tail tick and the per-row subagent lifecycle helpers.

use super::ClaudeEvent;
use super::adapter::{SubagentTail, TailState};
use super::background_tasks::{BackgroundOut, BackgroundState, now_ms, reconcile_background};
use super::persist::persist_extracted;
use crate::harness_actions_claude::ExtractedAction;
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom};

/// Subagent-transcript half of `tick`: walks every `agent-*.jsonl`
/// under `state.subagents_dir`, tailing each exactly like the main
/// file (byte offset + carried partial line), and appends any
/// `SubagentStart`/`SubagentToolResult`/`SubagentEnd` events to
/// `events`. Called under the same lock `tick` already holds — never
/// locks independently.
///
/// A `None` `subagents_dir` (creation or watch failed at attach, or
/// this session never delegated and the dir was never armed) is a
/// silent no-op: subagent telemetry is strictly additive and must
/// never affect the main tail.
///
/// Also persists (and, live, broadcasts) one `subagent_end`
/// `harness_actions` row per `SubagentEnd` — mirroring how `tick`'s
/// main-file loop feeds `s.actions` — so the Live Context feed gets a
/// "subagent finished" row (epic #298). This only ever runs from a
/// live tick, never from attach-time disk seeding: a subagent already
/// finished on disk when `attach_at` seeds `initial_subagents` never
/// enters this function's terminal-row branch at all, because its
/// `finished` flag is already `true` and its rows are never re-read.
///
/// Every subagent row is also fed to the background tracker (#445), which
/// appends its `BackgroundStart`/`BackgroundEnd` events to `events` and
/// its `background_end` rows to `background_rows` (persisted by `tick`);
/// a subagent finishing ends the tasks it launched.
pub(super) fn tick_subagents(
    s: &mut TailState,
    events: &mut Vec<ClaudeEvent>,
    background_rows: &mut Vec<ExtractedAction>,
) {
    let Some(dir) = s.subagents_dir.clone() else {
        return;
    };
    let entries = match fs::read_dir(&dir) {
        Ok(e) => {
            if s.subagents_dir_read_failure_logged {
                s.subagents_dir_read_failure_logged = false;
                tracing::debug!(
                    harness_id = %s.harness_id,
                    dir = %dir.display(),
                    "claude_events: subagents dir readable again"
                );
            }
            e
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // #362: normal now that `attach_at` no longer pre-creates
            // this directory — most sessions never delegate at all, so
            // this is the common case, not a problem to log. Leaves
            // `subagents_dir_read_failure_logged` untouched either way:
            // a prior *real* failure (permissions, say) still gets its
            // "readable again" debug line once a later read actually
            // succeeds, and this NotFound tick doesn't count as that.
            return;
        }
        Err(e) => {
            if !s.subagents_dir_read_failure_logged {
                s.subagents_dir_read_failure_logged = true;
                tracing::warn!(
                    harness_id = %s.harness_id,
                    dir = %dir.display(),
                    error = %e,
                    "claude_events: could not read subagents dir this tick"
                );
            }
            return;
        }
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut new_count: usize = 0;
    let mut reseeded = false;
    let mut subagent_end_actions: Vec<crate::harness_actions_claude::ExtractedAction> = Vec::new();
    for entry in entries.flatten() {
        let sub_path = entry.path();
        let Some(agent_id) = skein_harness::claude::subagent_id_from_path(&sub_path) else {
            continue;
        };
        seen.insert(agent_id.clone());

        let first_sighting = !s.subagents.contains_key(&agent_id);
        if first_sighting {
            // A transcript we haven't seen before — a fresh
            // delegation since attach (or since the last tick).
            new_count += 1;
            let meta = skein_harness::claude::read_subagent_meta(&sub_path);
            let meta_settled = meta.is_some();
            let (agent_type, description) =
                meta.map_or((None, None), |m| (m.agent_type, m.description));
            // #362: one line per subagent id the moment it joins the
            // tailed set — the discovery this issue's symptom traced
            // back to (a session that had just started delegating).
            tracing::info!(
                harness_id = %s.harness_id,
                agent_id = %agent_id,
                initial = false,
                agent_type = ?agent_type,
                "claude_events: subagent transcript joined the tailed set"
            );
            events.push(ClaudeEvent::SubagentStart {
                agent_id: agent_id.clone(),
                agent_type: agent_type.clone(),
                description: description.clone(),
                initial: false,
            });
            s.subagents.insert(
                agent_id.clone(),
                SubagentTail {
                    path: sub_path.clone(),
                    last_pos: 0,
                    partial: String::new(),
                    lifecycle: skein_harness::claude::SubagentLifecycle::default(),
                    agent_type,
                    meta_settled,
                    description,
                    started_ms: None,
                    started_ms_resolved: false,
                    last_ts_ms: 0,
                    open_failure_logged: false,
                },
            );
        }

        let Some(tail) = s.subagents.get_mut(&agent_id) else {
            continue;
        };

        // #497: the sidecar can land (or finish being written) after
        // the transcript is first seen. Re-read it while either label
        // field is still unknown — before the stat-skip below, since
        // the sidecar can arrive while the transcript is quiet. Once
        // a read parses, or the subagent finishes, it is not read again.
        if !first_sighting && let Some(label) = refresh_label(tail, &agent_id) {
            events.push(label);
        }

        // `tick` fires on ANY watched-path change — including ordinary
        // main-transcript writes that have nothing to do with
        // subagents — and finished entries are never removed from
        // `s.subagents` (the re-open transition below depends on them
        // surviving). Left unchecked, a long session with many
        // delegations pays an open+seek+read for every quiet subagent
        // file on every single tick, all under the tail lock. Stat
        // first and skip straight to the next entry when the file's
        // length hasn't moved since we last read it — a `stat` is far
        // cheaper than an `open`+`read`, and it helps a live-but-quiet
        // tail just as much as a finished one. A stat failure is not
        // proof there's nothing new, so it falls through to the normal
        // open path below rather than skipping — a skip must never
        // cost us data.
        if let Ok(meta) = fs::metadata(&tail.path)
            && meta.len() == tail.last_pos
        {
            continue;
        }

        let mut file = match fs::File::open(&tail.path) {
            Ok(f) => f,
            Err(e) => {
                if !tail.open_failure_logged {
                    tail.open_failure_logged = true;
                    tracing::warn!(
                        harness_id = %s.harness_id,
                        path = %tail.path.display(),
                        error = %e,
                        "claude_events: could not open subagent transcript"
                    );
                }
                continue;
            }
        };
        if tail.open_failure_logged {
            tail.open_failure_logged = false;
            tracing::debug!(
                harness_id = %s.harness_id,
                path = %tail.path.display(),
                "claude_events: subagent transcript readable again"
            );
        }
        if let Ok(meta) = file.metadata()
            && meta.len() < tail.last_pos
        {
            // #425: re-seed the way `attach_at` seeds a subagent — jump
            // to EOF, no events, no rows. Replaying from 0 would
            // re-emit rows already seen as live activity.
            // An unreadable file (a write cut mid UTF-8 char) is left
            // for the next tick: never fall back to 0, which would
            // replay this subagent live.
            let Ok(content) = fs::read_to_string(&tail.path) else {
                continue;
            };
            tail.last_pos = u64::try_from(content.len()).unwrap_or(u64::MAX);
            tail.partial.clear();
            // The tracker takes the content again with no events: ids it
            // already knows are ignored, and nothing here is replayed.
            // What that changed is reconciled against what was announced
            // once the loop is done (`reseeded`).
            reseeded = true;
            tail.lifecycle =
                subagent_lifecycle_from_content(&content, &mut s.background, &agent_id);
            tracing::info!(
                harness_id = %s.harness_id,
                path = %tail.path.display(),
                last_pos = tail.last_pos,
                file_len = meta.len(),
                "claude_events: subagent transcript shrank; re-seeded without replay"
            );
            continue;
        }
        if let Err(e) = file.seek(SeekFrom::Start(tail.last_pos)) {
            if !tail.open_failure_logged {
                tail.open_failure_logged = true;
                tracing::warn!(
                    harness_id = %s.harness_id,
                    path = %tail.path.display(),
                    error = %e,
                    "claude_events: subagent seek failed"
                );
            }
            continue;
        }
        let mut buf = String::new();
        let Ok(bytes) = file.read_to_string(&mut buf) else {
            // UTF-8 decode failed somewhere mid-file — same story as
            // the main tail's identical guard in `tick`: we landed
            // mid multi-byte char. Leave `last_pos` where it is and
            // wait for the next tick to pick up a full line; logged
            // at trace so it's diagnosable without being noisy.
            tracing::trace!(path = %tail.path.display(), "claude_events: subagent utf8 mid-line; retrying next tick");
            continue;
        };
        let advance: u64 = u64::try_from(bytes).unwrap_or(u64::MAX);
        tail.last_pos = tail.last_pos.saturating_add(advance);

        tail.partial.push_str(&buf);
        let drained = std::mem::take(&mut tail.partial);
        let mut lines = drained.split('\n').peekable();
        while let Some(line) = lines.next() {
            if lines.peek().is_none() {
                tail.partial.push_str(line);
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
                continue;
            };
            let row_ts = skein_harness::claude::timestamp_ms(&value);
            if !tail.started_ms_resolved {
                tail.started_ms = row_ts;
                tail.started_ms_resolved = true;
            }
            if let Some(ts) = row_ts {
                tail.last_ts_ms = ts;
            }
            // The lifecycle decides exit and re-open (#440). Behaviour
            // change: only a user prompt row (no tool_result) re-opens a
            // finished subagent now. Assistant rows and non-message rows
            // (e.g. a trailing `attachment`) after an exit used to
            // re-open it, which misfired on the 2.1.27x trailing rows.
            // The exit is a `SubagentHandback` tool_result (report
            // delivered; Claude marks it `toolEndsTurn`), not its tool_use.
            s.background.feed(
                &value,
                Some(&agent_id),
                Some(&mut BackgroundOut {
                    harness_id: &s.harness_id,
                    events,
                    rows: background_rows,
                }),
            );
            match tail.lifecycle.observe(&value) {
                Some(skein_harness::claude::SubagentTransition::Finished) => {
                    events.push(ClaudeEvent::SubagentEnd {
                        agent_id: agent_id.clone(),
                        agent_type: tail.agent_type.clone(),
                        description: tail.description.clone(),
                    });
                    // Its background tasks die with it (#445); Skein
                    // can't know whether they finished, so events only.
                    s.background.end_owned_by(&agent_id, events);
                    // Feed a `subagent_end` row into the activity feed
                    // (epic #298). The delegation itself already
                    // renders via the main transcript's `Agent`
                    // tool_call row (`AgentRow`, toolRows.tsx) — what's
                    // missing is the other end for a *background*
                    // subagent: its `AgentRow` lands at launch
                    // (`toolUseResult.status == "async_launched"`) and
                    // nothing ever marks completion. A second
                    // "delegated" row would just duplicate the
                    // existing one, so this is "finished" only.
                    let duration_ms = tail
                        .started_ms
                        .map(|start| tail.last_ts_ms.saturating_sub(start));
                    let mut payload = serde_json::json!({
                        "agent_id": agent_id,
                        "agent_type": tail.agent_type,
                        "description": tail.description,
                    });
                    if let Some(duration_ms) = duration_ms
                        && let Some(obj) = payload.as_object_mut()
                    {
                        obj.insert("duration_ms".into(), serde_json::json!(duration_ms));
                    }
                    subagent_end_actions.push(crate::harness_actions_claude::ExtractedAction {
                        kind: crate::db::action_kind::SUBAGENT_END,
                        timestamp_ms: tail.last_ts_ms,
                        payload: payload.to_string(),
                        source: None,
                    });
                }
                Some(skein_harness::claude::SubagentTransition::Reopened) => {
                    // A new prompt arrived after the exit — a
                    // follow-up delegation to the same id. Live again.
                    // Reuses the cached `agent_type`/`description` (kept
                    // current by `refresh_label` until both are known)
                    // rather than re-reading the `.meta.json` sidecar —
                    // on the assumption Claude never changes a known
                    // field after the fact.
                    events.push(ClaudeEvent::SubagentStart {
                        agent_id: agent_id.clone(),
                        agent_type: tail.agent_type.clone(),
                        description: tail.description.clone(),
                        initial: false,
                    });
                }
                None => {}
            }
            if is_subagent_tool_result_row(&value) {
                events.push(ClaudeEvent::SubagentToolResult {
                    agent_id: agent_id.clone(),
                });
            }
        }
    }

    // A transcript that vanished (pruned/rotated — not observed in
    // practice, but defensive symmetry with the main tail): drop it
    // from the map, no event. Nothing downstream needs to be told a
    // file disappeared; the last event it emitted already said
    // whether it was live or finished.
    s.subagents.retain(|id, _| seen.contains(id));

    if reseeded {
        events.extend(reconcile_background(
            &mut s.background,
            &s.subagents,
            now_ms(),
        ));
    }

    tracing::debug!(
        harness_id = %s.harness_id,
        seen = seen.len(),
        new = new_count,
        "claude_events: tick_subagents"
    );

    // Persist (and, live, broadcast) any `subagent_end` rows collected
    // above — same sink and same `emit` semantics as the main tail's
    // `persist_extracted(ap, extracted, true)` call in `tick.rs`. `None`
    // for the phase-only test constructor, in which case this is a
    // silent no-op (nothing to persist to).
    if let Some(ap) = s.actions.as_ref() {
        persist_extracted(ap, subagent_end_actions, true);
    }
}

/// Runs a fresh `SubagentLifecycle` over every complete line in
/// `content`. Used to seed a subagent's lifecycle from what's already
/// on disk at attach time, mirroring the per-row logic `tick_subagents`
/// applies while tailing live. The lifecycle is kept (not just its
/// verdict) so a `SubagentHandback` id seen before attach still matches
/// a result that arrives afterwards.
///
/// The same walk feeds `background` with the subagent's rows (#445), no
/// events: history is not replayed, but the tracker has to know which
/// tasks this subagent started and owns.
pub(super) fn subagent_lifecycle_from_content(
    content: &str,
    background: &mut BackgroundState,
    agent_id: &str,
) -> skein_harness::claude::SubagentLifecycle {
    let mut lifecycle = skein_harness::claude::SubagentLifecycle::default();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        lifecycle.observe(&value);
        background.feed(&value, Some(agent_id), None);
    }
    lifecycle
}

/// A `user` row whose `message.content` carries a `tool_result` block
/// — a tool call inside a subagent's own turn just returned. This is
/// what backs `ClaudeEvent::SubagentToolResult`; see its doc comment
/// for why it exists and what it must not be read as.
pub(super) fn is_subagent_tool_result_row(value: &serde_json::Value) -> bool {
    if value.get("type").and_then(serde_json::Value::as_str) != Some("user") {
        return false;
    }
    value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|blocks| {
            blocks
                .iter()
                .any(|b| b.get("type").and_then(serde_json::Value::as_str) == Some("tool_result"))
        })
}

/// #497: fill in whichever of `agent_type`/`description` is still
/// `None` from the sidecar, until one read parses (`meta_settled`): a
/// valid sidecar may legitimately lack a field, so a parse is final.
/// Never overwrites a known value, and returns the full current label
/// as a `SubagentLabel` only when something changed. A missing or
/// half-written sidecar reads as `None` and is retried on the next
/// tick, but only while the subagent is live: a finished one stops
/// being read (a reopen makes it live, and retried, again).
fn refresh_label(tail: &mut SubagentTail, agent_id: &str) -> Option<ClaudeEvent> {
    if tail.meta_settled || tail.lifecycle.is_finished() {
        return None;
    }
    let meta = skein_harness::claude::read_subagent_meta(&tail.path)?;
    tail.meta_settled = true;
    let mut changed = false;
    if tail.agent_type.is_none() && meta.agent_type.is_some() {
        tail.agent_type = meta.agent_type;
        changed = true;
    }
    if tail.description.is_none() && meta.description.is_some() {
        tail.description = meta.description;
        changed = true;
    }
    changed.then(|| ClaudeEvent::SubagentLabel {
        agent_id: agent_id.to_owned(),
        agent_type: tail.agent_type.clone(),
        description: tail.description.clone(),
    })
}
