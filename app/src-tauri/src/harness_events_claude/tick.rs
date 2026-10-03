//! The main-transcript tail tick: read new bytes, parse rows, dispatch events, heartbeat.

use super::ClaudeEvent;
use super::adapter::TailState;
use super::background_tasks::{BackgroundOut, now_ms};
use super::cli_version::live_cli_version_event;
use super::parse::parse_value;
use super::persist::persist_extracted;
use super::resync::{Reappear, classify_reappear, extend_fingerprint, finish_resync};
use super::subagents::tick_subagents;
use super::supervise::panic_message;
use crate::harness_actions_claude::ExtractedAction;
use parking_lot::Mutex;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Tighter than the worktree watcher's 200 ms — notification UX cares
/// about latency, and a JSONL append produces exactly one event we
/// want to react to quickly.
pub(super) const DEBOUNCE_MS: u64 = 50;

/// Minimum spacing between heartbeat log lines for one attached adapter
/// (#362). Piggybacks on whatever tick already ran — no timer thread of
/// its own, so a transcript with nothing nearby to trigger a tick
/// simply gets no heartbeat until the next one does. That's fine: the
/// heartbeat's job is to prove "the Rust side is still ticking", and a
/// rising `last_pos` alongside `events_sent` is what distinguishes that
/// from "Rust stalled" — a frontend-silent-but-Rust-healthy report
/// would show both climbing while nothing arrives on the other side.
pub(super) const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);

/// How many consecutive ticks may see a UTF-8 decode failure at the
/// *same* `last_pos` before it's worth a warn (#362). One or two is the
/// ordinary case — a write straddling a debounce tick, picked up whole
/// on the very next one — logged at debug and cleared automatically the
/// moment a read at that position succeeds. A run that survives this
/// many ticks (1s+ of wall time at the 50ms debounce) means the file is
/// stuck mid multi-byte character for good, not just a momentary race.
pub(super) const UTF8_STALL_WARN_THRESHOLD: u32 = 20;

/// Pure decision for whether a heartbeat should fire this tick, given
/// when the last one fired (`None` = never yet) and the current time.
/// Factored out so the rate limit is testable without sleeping 60s
/// (#362) — a test just constructs `now` and `now - INTERVAL - epsilon`.
pub(super) fn should_heartbeat(last: Option<Instant>, now: Instant) -> bool {
    match last {
        None => true,
        Some(last) => now.saturating_duration_since(last) >= HEARTBEAT_INTERVAL,
    }
}

/// Pure decision for whether this tick should log the UTF-8 stall warn,
/// given the running consecutive-failure count (already incremented for
/// this tick) and whether the warn already fired for the current run.
/// Fires exactly once per stall run, on the tick where the count first
/// reaches [`UTF8_STALL_WARN_THRESHOLD`] — factored out for the same
/// clock-free testability reason as `should_heartbeat`.
pub(super) fn should_warn_utf8_stall(count: u32, already_warned: bool) -> bool {
    count >= UTF8_STALL_WARN_THRESHOLD && !already_warned
}

/// One tick of the tail-reader. Reads any bytes appended since
/// `last_pos`, splits into lines, parses each as a Claude event, and
/// emits `ClaudeEvent`s. Called from the debouncer's flush thread.
pub(super) fn tick(state: &Arc<Mutex<TailState>>, on_event: &(dyn Fn(ClaudeEvent) + Send + Sync)) {
    let mut s = state.lock();
    // If we previously hadn't attached (file didn't exist), check
    // again. The watcher fires for any change in the parent dir, so
    // this is the moment we'd notice the create.
    if !s.attached {
        if s.path.exists() {
            s.attached = true;
            if s.ever_attached {
                // #425: the transcript we had already consumed
                // vanished and is back. Replaying it from 0 as live
                // would re-emit every historic row (phase events,
                // broadcasts, review baselines). Decide whether it is
                // the same file; if not, resync it as backfill.
                let file_len = fs::metadata(&s.path).map_or(0, |m| m.len());
                let case = classify_reappear(&s.path, s.last_pos, &s.fingerprint, file_len);
                tracing::info!(
                    harness_id = %s.harness_id,
                    path = %s.path.display(),
                    case = case.name(),
                    last_pos = s.last_pos,
                    file_len,
                    "claude_events: transcript reappeared"
                );
                if case != Reappear::Same {
                    finish_resync(s, state, on_event);
                    return;
                }
            } else {
                s.ever_attached = true;
                // Start at 0 — file is fresh, all bytes are new.
                s.last_pos = 0;
                s.fingerprint.clear();
                tracing::info!(
                    harness_id = %s.harness_id,
                    path = %s.path.display(),
                    "claude_events: transcript appeared"
                );
            }
        } else {
            // Still not there. Maybe the watcher fired for an
            // unrelated file in the project dir; keep waiting.
            return;
        }
    }

    // Stat first and skip the open when nothing has been appended
    // (#428). notify 8.2's inotify backend arms IN_OPEN on watched
    // dirs and notify-debouncer-mini forwards every event kind, so on
    // Linux this tick's own `File::open` would queue the next tick
    // 50 ms later, a self-sustaining loop for the adapter's lifetime.
    // Windows/macOS don't report opens, which is why it only shows on
    // Linux. A stat failure falls through to the open path, so a
    // vanished file still emits SessionEnd; shrink and growth also
    // fall through.
    let unchanged_len = fs::metadata(&s.path)
        .ok()
        .map(|m| m.len())
        .filter(|&len| len == s.last_pos);
    let (buf, bytes, file_len) = if let Some(len) = unchanged_len {
        (String::new(), 0, Some(len))
    } else {
        // Open + seek + read-to-end. Rolling a long-lived File handle
        // would be a tiny optimisation but risks holding a stale fd if
        // Claude ever rotates the file. Reopening every tick is robust
        // and the file sizes we're dealing with (kilobytes of JSON per
        // event) are trivial to seek into.
        let mut file = match fs::File::open(&s.path) {
            Ok(f) => f,
            Err(e) => {
                // File vanished — Claude was uninstalled mid-session, or
                // the user wiped ~/.claude. Surface as SessionEnd once and
                // detach from this run (the adapter stays alive in case the
                // file comes back, but we don't keep re-emitting).
                if s.attached {
                    s.attached = false;
                    let harness_id = s.harness_id.clone();
                    let path = s.path.clone();
                    drop(s);
                    tracing::warn!(
                        harness_id = %harness_id,
                        path = %path.display(),
                        error = %e,
                        "claude_events: transcript vanished; emitting SessionEnd"
                    );
                    on_event(ClaudeEvent::SessionEnd);
                } else if e.kind() != std::io::ErrorKind::NotFound {
                    // NotFound while `!attached` is the expected "still
                    // waiting for Claude to write the first row" case —
                    // anything else (permissions, IO error) is worth a
                    // line since it means this harness may never attach.
                    tracing::warn!(
                        harness_id = %s.harness_id,
                        path = %s.path.display(),
                        error = %e,
                        "claude_events: unexpected error opening transcript"
                    );
                }
                return;
            }
        };

        // Defensive: file size dropped below last_pos (rotation /
        // truncation). #425: resync as backfill rather than replaying from
        // 0 as live — the rows in it are history we already reported (or
        // that the DB already holds), not new events.
        if let Ok(meta) = file.metadata()
            && meta.len() < s.last_pos
        {
            tracing::info!(
                harness_id = %s.harness_id,
                path = %s.path.display(),
                case = Reappear::Truncated.name(),
                last_pos = s.last_pos,
                file_len = meta.len(),
                "claude_events: transcript shrank"
            );
            drop(file);
            finish_resync(s, state, on_event);
            return;
        }

        if let Err(e) = file.seek(SeekFrom::Start(s.last_pos)) {
            tracing::warn!(
                harness_id = %s.harness_id,
                path = %s.path.display(),
                pos = s.last_pos,
                error = %e,
                "claude_events: seek failed"
            );
            return;
        }
        let mut buf = String::new();
        let bytes = match file.read_to_string(&mut buf) {
            Ok(b) => b,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::InvalidData {
                    // UTF-8 decode failed somewhere mid-file. JSONL is
                    // ASCII for the keys + UTF-8 content; the only way
                    // this fires is if we landed in the middle of a
                    // multi-byte char. Bump last_pos by what we did read
                    // (zero in this case) and wait for the next tick to
                    // pick up a full line. Expected transient state, not
                    // an error — logged at debug.
                    tracing::debug!(
                        harness_id = %s.harness_id,
                        path = %s.path.display(),
                        "claude_events: utf8 mid-line; retrying next tick"
                    );
                    // #362: a one- or two-tick stall here is the ordinary
                    // case above. Count consecutive failures at the SAME
                    // last_pos — a run that survives past the threshold
                    // means the file is stuck mid multi-byte character for
                    // good, not just a write straddling a debounce tick.
                    if s.utf8_stall_at == Some(s.last_pos) {
                        s.utf8_stall_count = s.utf8_stall_count.saturating_add(1);
                    } else {
                        s.utf8_stall_at = Some(s.last_pos);
                        s.utf8_stall_count = 1;
                        s.utf8_stall_warned = false;
                    }
                    if should_warn_utf8_stall(s.utf8_stall_count, s.utf8_stall_warned) {
                        s.utf8_stall_warned = true;
                        let len = file.metadata().ok().map(|m| m.len());
                        tracing::warn!(
                            harness_id = %s.harness_id,
                            path = %s.path.display(),
                            pos = s.last_pos,
                            file_len = ?len,
                            ticks = s.utf8_stall_count,
                            "claude_events: transcript stuck mid multi-byte character across many ticks"
                        );
                    }
                } else {
                    tracing::warn!(
                        harness_id = %s.harness_id,
                        path = %s.path.display(),
                        error = %e,
                        "claude_events: read failed"
                    );
                }
                return;
            }
        };
        (buf, bytes, file.metadata().ok().map(|m| m.len()))
    };
    // Reaching here means the read decoded cleanly — any UTF-8 stall
    // run in progress is over. Rearm so a *future* stall gets its own
    // fresh count and its own warn (#362).
    s.utf8_stall_at = None;
    s.utf8_stall_count = 0;
    s.utf8_stall_warned = false;
    let advance: u64 = u64::try_from(bytes).unwrap_or(u64::MAX);
    s.last_pos = s.last_pos.saturating_add(advance);
    extend_fingerprint(&mut s.fingerprint, buf.as_bytes());

    // First bytes ever read off the MAIN transcript since this attach
    // (#362) — once per attach, logged at info so "attached but the
    // watcher never fires / nothing read" is distinguishable from
    // "tailing fine" without cranking RUST_LOG. Fires on the first
    // *appended* bytes for a resumed session too, since `attach_at`
    // seeks straight to EOF before this tick ever runs.
    if bytes > 0 && !s.first_read_logged {
        s.first_read_logged = true;
        tracing::info!(
            harness_id = %s.harness_id,
            bytes,
            "claude_events: first transcript bytes read since attach"
        );
    }

    // Prepend any leftover partial line from last tick, then split on
    // newlines. The trailing chunk (anything after the last '\n') is
    // partial — carry it forward.
    s.partial.push_str(&buf);
    // Drain the partial buffer locally so we don't borrow `s.partial`
    // while we walk lines (and also so the next tick starts clean).
    let drained = std::mem::take(&mut s.partial);
    let mut lines = drained.split('\n').peekable();
    let mut events = Vec::new();
    let mut background_rows: Vec<ExtractedAction> = Vec::new();
    let harness_id = s.harness_id.clone();
    let mut in_assistant_turn = s.in_assistant_turn;
    let mut local_command = std::mem::take(&mut s.local_command);
    let mut lines_parsed: usize = 0;
    while let Some(line) = lines.next() {
        if lines.peek().is_none() {
            // Final segment — either the last line was incomplete
            // (no trailing newline) and this is the partial to
            // carry forward, or the input ended with '\n' and this
            // is an empty string. Either way, stash it.
            s.partial.push_str(line);
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        lines_parsed += 1;
        // Parse once, fan out to both consumers: phase (the
        // pre-existing path) and actions (issue #80). Action
        // persistence is best-effort — a single failed insert
        // shouldn't kill the tail loop.
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        if let Some(event) = parse_value(&value, &mut in_assistant_turn, &mut local_command) {
            events.push(event);
        }
        if let Some(event) = live_cli_version_event(&value, &mut s.last_cli_version) {
            events.push(event);
        }
        if let Some(ap) = s.actions.as_mut() {
            let extracted = ap.extractor.ingest(&value);
            persist_extracted(ap, extracted, true);
        }
        // Background tasks (#445): the same row, a third consumer.
        s.background.feed(
            &value,
            None,
            Some(&mut BackgroundOut {
                harness_id: &harness_id,
                events: &mut events,
                rows: &mut background_rows,
            }),
        );
    }
    s.in_assistant_turn = in_assistant_turn;
    s.local_command = local_command;

    // Subagent transcripts, after the main file — same lock, same
    // events vec, so consumers see main-session events first and
    // subagent events second within one tick.
    let main_events = events.len();
    tick_subagents(&mut s, &mut events, &mut background_rows);
    // Monitor deadlines, after both files so a real expiry notice that
    // arrived this tick wins over the sweep (#445).
    s.background.sweep(
        now_ms(),
        &mut BackgroundOut {
            harness_id: &harness_id,
            events: &mut events,
            rows: &mut background_rows,
        },
    );
    if let Some(ap) = s.actions.as_ref() {
        persist_extracted(ap, background_rows, true);
    }
    let subagent_events = events.len() - main_events;
    // Only the subagents still being tailed live count as "watched" —
    // a finished one is a cheap stat check, not a file whose new rows
    // we're expecting (#362).
    let live_subagents = s
        .subagents
        .values()
        .filter(|t| !t.lifecycle.is_finished())
        .count();
    let watched_files = 1 + live_subagents;
    tracing::debug!(
        harness_id = %s.harness_id,
        bytes,
        lines_parsed,
        events = main_events,
        subagent_events,
        watched_files,
        "claude_events: tick"
    );

    // Heartbeat (#362): at most once per HEARTBEAT_INTERVAL per
    // attached adapter, and only on a tick that actually runs — no
    // timer thread. A rising `last_pos` alongside `events_sent` proves
    // the Rust side is still reading and emitting even when the
    // frontend goes quiet; both frozen while `file_len` keeps growing
    // is what an actual stall looks like.
    let now = Instant::now();
    if should_heartbeat(s.last_heartbeat, now) {
        s.last_heartbeat = Some(now);
        tracing::info!(
            harness_id = %s.harness_id,
            session = %s.path.display(),
            last_pos = s.last_pos,
            file_len = ?file_len,
            events_sent = s.events_sent,
            send_errors = s.send_errors,
            live_subagents,
            "claude_events: heartbeat"
        );
    }
    drop(s);

    dispatch_events(state, events, on_event);
}

/// Hand `events` to `on_event` with tick's panic containment and
/// `events_sent`/`send_errors` bookkeeping. Must be called WITHOUT the
/// state lock held. Shared by `tick` and the #425 resync path so a
/// resync's initial event is counted like any other.
pub(super) fn dispatch_events(
    state: &Arc<Mutex<TailState>>,
    events: Vec<ClaudeEvent>,
    on_event: &(dyn Fn(ClaudeEvent) + Send + Sync),
) {
    // A panic inside `on_event` would otherwise unwind straight through
    // `tick` — caught here (rather than only by the debouncer callback's
    // own `catch_unwind`) so it's counted toward `send_errors` and
    // logged with the harness id that triggered it (#362). The
    // production callback in `lib.rs` never panics — it only
    // `.is_err()`s a dead channel — so this is expected to stay silent
    // there; it exists for this counter's own honesty and for tests
    // that use a channel that can panic on send.
    //
    // `dispatched` counts only events actually handed to `on_event`
    // before any panic, matching `events_sent`'s own doc — a panic
    // partway through the loop must not credit events that were never
    // delivered.
    let mut dispatched: u64 = 0;
    let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for event in events {
            on_event(event);
            dispatched += 1;
        }
    }));
    let mut s = state.lock();
    s.events_sent = s.events_sent.saturating_add(dispatched);
    if let Err(panic) = panic_result {
        s.send_errors = s.send_errors.saturating_add(1);
        tracing::error!(
            harness_id = %s.harness_id,
            panic = %panic_message(&panic),
            total_send_errors = s.send_errors,
            "claude_events: on_event panicked while dispatching this tick's events"
        );
    }
}
