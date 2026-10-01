//! Building an adapter from scratch and installing it, for a fresh attach and for a reattach.

use super::adapter::{
    ActionPersistence, Adapter, AttachInfo, ReattachRecipe, Registry, SubagentTail, TailState,
};
use super::background_tasks::{BackgroundState, now_ms, reconcile_background};
use super::paths::{DirId, desired_watches};
use super::persist::{persist_extracted_batch, scan_history};
use super::resync::fingerprint_of;
use super::subagents::subagent_lifecycle_from_content;
use super::supervise::panic_message;
use super::tick::{DEBOUNCE_MS, tick};
use super::{ClaudeEvent, ClaudeEventsError};
use notify_debouncer_mini::notify::RecursiveMode;
use notify_debouncer_mini::{DebounceEventResult, new_debouncer};
use parking_lot::Mutex;
use skein_harness::claude::local_command::LocalCommandTracker;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Builds a fully-armed `Adapter` from scratch — all the real work of
/// an attach (reading history, arming watches, running the first
/// catch-up tick) — WITHOUT ever touching `Registry` (#410 review fix).
/// That split matters: installing the result needs a
/// compare-and-swap-under-lock for a reattach (see `reattach_at_impl`)
/// but not for a normal caller-driven attach (see `attach_at_impl`),
/// and neither install step should hold the registry lock across this
/// function's disk I/O and `notify` calls. `Adapter::generation` on the
/// returned value is a placeholder (`0`) — every caller overwrites it
/// with a real generation at the moment it actually installs the
/// adapter, under the lock.
pub(super) fn build_adapter<F>(
    harness_id: &str,
    path: PathBuf,
    on_event: F,
    actions: Option<ActionPersistence>,
) -> Result<(Adapter, AttachInfo), ClaudeEventsError>
where
    F: Fn(ClaudeEvent) + Send + Sync + 'static,
{
    {
        // Determine starting position. Three cases:
        //   • File doesn't exist (fresh spawn before Claude has
        //     written anything) — start at 0; watcher's create event
        //     will trip the attached=true branch in `tick`.
        //   • File exists (resume after Skein restart) — read it
        //     once to derive the *current* phase, then seek to EOF
        //     for live tailing. Without this probe, Claude harnesses
        //     all show green on restart because the existing
        //     end_turn row was already in the file before we
        //     attached, so we never see it via tail. Bug found in
        //     v0.1.9-dev — see `parse::determine_initial_state`.
        //   • Error other than NotFound — treat as "fresh" (file
        //     will appear later, or it really doesn't exist).
        // Backfill (issue #80): before seeking to EOF for live tail,
        // extract every action from the file's existing content and
        // persist anything newer than what we've already seen. On
        // first-ever attach for this harness, that's the whole file;
        // on re-attach after Skein restart, we only insert rows newer
        // than the largest persisted timestamp. The phase-side
        // initial-event probe is unchanged; both consume the same
        // file read — `scan_history` walks it exactly once (#171e:
        // this used to be two full parses, `determine_initial_state`
        // then `backfill_actions`, each running `serde_json::from_str`
        // on every line) and the fresh actions it collects are
        // persisted with one batch insert instead of one per line.
        let mut actions = actions;
        // Captured before `actions` is moved into `TailState` below, so
        // a future re-attach (#410) can build a brand-new
        // `ActionPersistence` — including a fresh `ActionExtractor` —
        // rather than resurrecting one that may be stuck mid-turn.
        let reattach_recipe = actions.as_ref().map(|ap| ReattachRecipe {
            db: Arc::clone(&ap.db),
            harness_id: ap.harness_id.clone(),
            room_id: ap.room_id.clone(),
            cwd: ap.cwd.clone(),
            app: ap.app.clone(),
        });
        let mut background = BackgroundState::default();
        let mut local_command = LocalCommandTracker::default();
        let (last_pos, attached, initial_event, fingerprint) = match fs::read_to_string(&path) {
            Ok(content) => {
                let (init, fresh) = scan_history(
                    &content,
                    actions.as_mut(),
                    &mut background,
                    &mut local_command,
                );
                if let Some(ap) = actions.as_ref() {
                    persist_extracted_batch(ap, fresh);
                }
                let len = u64::try_from(content.len()).unwrap_or(u64::MAX);
                (len, true, init, fingerprint_of(content.as_bytes()))
            }
            // The file EXISTS but can't be read as UTF-8 (a write cut
            // mid-character): claim it as already attached-before, with
            // a `last_pos` no file can reach, so the first tick takes
            // the reappear path and resyncs it as backfill instead of
            // reading it from 0 as live (#425).
            Err(_) if path.exists() => (u64::MAX, false, None, Vec::new()),
            Err(_) => (0, false, None, Vec::new()),
        };
        let ever_attached = attached || last_pos == u64::MAX;
        // Derive the parent before moving `path` into TailState.
        // Watching the parent (not the file directly) survives some
        // platforms (Linux/inotify) losing the watch when the file is
        // replaced atomically. It's also `Adapter::rearm`'s (#362)
        // starting point for the desired watch set — see
        // `desired_watches`, which falls back to the nearest existing
        // ancestor when this doesn't exist yet (a brand-new worktree,
        // before Claude has written anything here at all).
        let parent = path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| ClaudeEventsError("session path has no parent".into()))?;

        // Subagent transcripts live in a sibling `<session-id>/subagents`
        // dir next to the main `.jsonl` (`skein_harness::claude::
        // subagents_dir`, #209). #362: this is NOT created here any
        // more — it's Claude's directory to create, the moment (if
        // ever) a session actually delegates, and pre-creating it ahead
        // of that was itself part of #362's bug: a watch armed on a
        // directory Skein invented, before anything could ever be
        // written into it. `subagents_dir_opt` is just the computed
        // path; whether it currently exists is checked fresh every time
        // `desired_watches` runs — at attach below, and on every later
        // supervisor pass (`Adapter::rearm`).
        let subagents_dir_opt = skein_harness::claude::subagents_dir(&path);

        // Seed initial subagent state from disk *before* arming the
        // watcher — same reasoning as the history probe above: the
        // disk is the truth, and a subagent could already be mid-flight
        // by the time we watch. `last_pos` starts at each file's
        // current length (everything already there counts as "seen");
        // `finished` reflects whichever row is last, so a subagent
        // that already finished before this attach doesn't get
        // replayed as freshly started. The `SubagentStart` events
        // themselves are held back to `initial_subagent_starts` and
        // emitted only after the watcher is armed, for the same
        // no-lost-window reason `initial_event` is below.
        //
        // Accepted gap: a subagent that started AND finished entirely
        // within the gap this attach is closing never gets a
        // `SubagentStart` *or* a `subagent_end` action row — it's
        // already `finished` by the time this runs, so it's seeded
        // silently, same as the identical gap on a plain Skein restart
        // (a PTY, and everything running in it, dies with the process).
        // #410 makes this reachable a second way: a dead-tail re-attach
        // runs this exact same seeding path, so a subagent that both
        // started and finished during the dead window is missed the
        // same way, even though the harness itself never restarted.
        let mut initial_subagents: HashMap<String, SubagentTail> = HashMap::new();
        let mut initial_subagent_starts: Vec<ClaudeEvent> = Vec::new();
        if let Some(dir) = &subagents_dir_opt
            && let Ok(entries) = fs::read_dir(dir)
        {
            for entry in entries.flatten() {
                let sub_path = entry.path();
                let Some(agent_id) = skein_harness::claude::subagent_id_from_path(&sub_path) else {
                    continue;
                };
                let content = fs::read_to_string(&sub_path).unwrap_or_default();
                let last_pos = u64::try_from(content.len()).unwrap_or(u64::MAX);
                let lifecycle =
                    subagent_lifecycle_from_content(&content, &mut background, &agent_id);
                let finished = lifecycle.is_finished();
                let meta = skein_harness::claude::read_subagent_meta(&sub_path);
                let (agent_type, description) =
                    meta.map_or((None, None), |m| (m.agent_type, m.description));
                if !finished {
                    // #362: one line per subagent id the moment it joins
                    // the tailed set, so a session with subagent activity
                    // shows up in the log even when the main transcript
                    // itself goes quiet around the same time.
                    tracing::info!(
                        harness_id = %harness_id,
                        agent_id = %agent_id,
                        initial = true,
                        agent_type = ?agent_type,
                        "claude_events: subagent transcript joined the tailed set"
                    );
                    initial_subagent_starts.push(ClaudeEvent::SubagentStart {
                        agent_id: agent_id.clone(),
                        agent_type: agent_type.clone(),
                        description: description.clone(),
                        initial: true,
                    });
                }
                initial_subagents.insert(
                    agent_id,
                    SubagentTail {
                        path: sub_path,
                        last_pos,
                        partial: String::new(),
                        lifecycle,
                        agent_type,
                        description,
                        // Seeded at attach, jumped straight to EOF —
                        // see the field doc on `started_ms`.
                        started_ms: None,
                        started_ms_resolved: true,
                        last_ts_ms: 0,
                        open_failure_logged: false,
                    },
                );
            }
        }

        // Background tasks (#445): main and subagent history are both
        // fed by now, so the live set can be derived (nothing announced
        // yet, so the result is all starts). Nothing the feeding
        // produced was kept — history is not replayed as events or rows.
        // What is still outstanding is held back like the subagent
        // starts, to be emitted once the watcher is armed.
        let initial_background_starts =
            reconcile_background(&mut background, &initial_subagents, now_ms());

        let attach_info_path = path.clone();
        let state = Arc::new(Mutex::new(TailState {
            harness_id: harness_id.to_string(),
            path,
            last_pos,
            partial: String::new(),
            attached,
            ever_attached,
            fingerprint,
            in_assistant_turn: false,
            // Seeded from the attach walk so a burst split across attach
            // and the first live read is still recognised (#463).
            local_command,
            actions,
            subagents_dir: subagents_dir_opt.clone(),
            subagents: initial_subagents,
            background,
            subagents_dir_read_failure_logged: false,
            first_read_logged: false,
            events_sent: 0,
            send_errors: 0,
            last_heartbeat: None,
            utf8_stall_at: None,
            utf8_stall_count: 0,
            utf8_stall_warned: false,
        }));
        let cb_state = Arc::clone(&state);
        let on_event: Arc<dyn Fn(ClaudeEvent) + Send + Sync> = Arc::new(on_event);
        let cb_on_event = Arc::clone(&on_event);
        let cb_harness_id = harness_id.to_string();

        let mut debouncer = new_debouncer(
            Duration::from_millis(DEBOUNCE_MS),
            move |result: DebounceEventResult| {
                // notify can deliver an Err when the queue overflows
                // (rare, but possible during heavy filesystem activity).
                // Treat that exactly like a normal tick — we'll read
                // up to the current EOF and catch up. Better
                // stale-but-honest than silently miss events. Still
                // worth a warn: a run of these means we may be missing
                // debounce coalescing, not just losing one event (#362).
                if let Err(errors) = &result {
                    tracing::warn!(
                        harness_id = %cb_harness_id,
                        ?errors,
                        "claude_events: debouncer reported error(s); ticking anyway"
                    );
                }
                // A panic inside `tick` would otherwise unwind straight
                // through the notify callback and silently kill the
                // watcher thread — no more events, ever, for this
                // harness, with nothing in the log to say why (#362).
                // `parking_lot::Mutex` doesn't poison, so the lock
                // stays usable for the next tick even if this one
                // panicked mid-hold.
                if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    tick(&cb_state, cb_on_event.as_ref());
                })) {
                    tracing::error!(
                        harness_id = %cb_harness_id,
                        panic = %panic_message(&panic),
                        "claude_events: tick panicked; watcher stays alive"
                    );
                }
            },
        )
        .map_err(ClaudeEventsError::from_err)?;

        // Arm the initial watch set (#362): `desired_watches` picks the
        // parent if it exists or its nearest existing ancestor if it
        // doesn't yet, plus the subagents dir when that already exists.
        // Neither is created here — see the doc comments above on
        // `parent`'s derivation and `subagents_dir_opt`. A watch that
        // fails to arm is warned and simply left out of `armed`; unlike
        // the old unconditional `?` on the main-parent watch, that must
        // NOT fail `attach` any more — the periodic supervisor
        // (`Adapter::rearm`) will retry it on the next pass, the same
        // path a directory that vanishes mid-session takes.
        let initial_desired = desired_watches(&parent, subagents_dir_opt.as_deref());
        let mut armed: Vec<(PathBuf, DirId)> = Vec::with_capacity(initial_desired.len());
        for (dir, id) in &initial_desired {
            match debouncer.watcher().watch(dir, RecursiveMode::NonRecursive) {
                Ok(()) => armed.push((dir.clone(), *id)),
                Err(e) => {
                    tracing::warn!(
                        harness_id = %harness_id,
                        path = %dir.display(),
                        error = %e,
                        "claude_events: could not arm watch at attach; the periodic supervisor will retry"
                    );
                }
            }
        }

        // Emit the synthetic initial event from the history probe
        // *after* arming the watcher — so if the file grows between
        // probe and arm, the tick that follows picks up the delta
        // (no race window where new events get lost). Emitting
        // before `tick` also means consumers see initial-state
        // first, live events second; that ordering matches their
        // expectation.
        if let Some(event) = initial_event {
            on_event(event);
        }
        // Same reasoning, for the subagents discovered above: emit
        // unconditionally — `initial_subagent_starts` is only ever
        // non-empty when `subagents_dir_opt` was `Some` *and* that dir
        // already existed and was readable (see the seeding loop
        // above), so there's nothing to gate here any more.
        for event in initial_subagent_starts {
            on_event(event);
        }
        for event in initial_background_starts {
            on_event(event);
        }

        // One immediate tick to catch anything written between the
        // read_to_string above and the watcher arming. In the steady
        // case this seeks to last_pos == file length and reads 0
        // bytes — cheap no-op.
        tick(&state, on_event.as_ref());

        let subagents_armed = subagents_dir_opt.is_some();
        let adapter = Adapter {
            debouncer,
            // The closure captured in the debouncer above holds its own
            // clone of each (`cb_state`/`cb_on_event`); these are a
            // third, used by `Adapter::rearm` and its catch-up tick
            // (#362), which run outside that closure entirely.
            state: Arc::clone(&state),
            on_event: Arc::clone(&on_event),
            armed,
            failing_watches: HashSet::new(),
            reattach_recipe,
            stall: None,
            // Overwritten by whichever install step (`attach_at_impl` or
            // `reattach_at_impl`) actually inserts this adapter — see
            // `Adapter::generation`'s doc comment.
            generation: 0,
        };
        drop(state);
        Ok((
            adapter,
            AttachInfo {
                path: attach_info_path,
                already_existed: attached,
                subagents_armed,
            },
        ))
    }
}

/// Install a freshly built adapter unconditionally (#410 review fix) —
/// the normal caller-driven path (`ClaudeEventsManager::attach`/
/// `attach_at`): a real `attach()` call always wins, replacing whatever
/// was there, exactly like before generations existed. Builds the
/// adapter (real I/O, `notify` calls) OUTSIDE the registry lock; only
/// the assign-generation-and-insert step is under it.
pub(super) fn attach_at_impl<F>(
    inner: &Arc<Mutex<Registry>>,
    harness_id: &str,
    path: PathBuf,
    on_event: F,
    actions: Option<ActionPersistence>,
) -> Result<AttachInfo, ClaudeEventsError>
where
    F: Fn(ClaudeEvent) + Send + Sync + 'static,
{
    let (mut adapter, info) = build_adapter(harness_id, path, on_event, actions)?;
    let mut reg = inner.lock();
    reg.next_generation += 1;
    adapter.generation = reg.next_generation;
    let replaced = reg
        .adapters
        .insert(harness_id.to_string(), adapter)
        .is_some();
    drop(reg);
    if replaced {
        tracing::info!(
            harness_id,
            "claude_events: attach replaced an existing adapter for this harness"
        );
    }
    Ok(info)
}

/// Outcome of `reattach_at_impl`'s compare-and-swap install (#410
/// review fix).
pub(super) enum ReattachInstall {
    /// The adapter this reattach built was installed. No payload —
    /// neither caller (`perform_reattach`, `ClaudeEventsManager::
    /// reattach`) needs anything from the `AttachInfo` a successful
    /// reattach produces, only the fact that it succeeded.
    Installed,
    /// The CAS failed: `harness_id`'s current generation no longer
    /// matched the one this reattach was decided against, because a
    /// `detach` or a fresh `attach`/another reattach landed first. The
    /// freshly built (unused) adapter — and the `notify` watches it
    /// armed — are simply dropped. `still_present` says which: `false`
    /// means detached (the harness is gone), `true` means replaced by
    /// something newer.
    Abandoned { still_present: bool },
}

/// Install a re-attached adapter under a compare-and-swap on
/// `Adapter::generation` (#410 review fix — the fix for a real race:
/// `supervise_map` collects dead-tail jobs, and `ClaudeEventsManager::
/// reattach` builds its replacement, entirely outside the registry
/// lock; either a `detach` or a fresh caller-driven `attach()` can land
/// on the same harness id before the replacement is ready). Only
/// installs when `harness_id`'s adapter STILL has `expected_generation`
/// — otherwise something else already resolved this harness id and the
/// reattach is simply abandoned, never resurrecting a closed harness or
/// clobbering a newer adapter. Builds the adapter OUTSIDE the lock,
/// same as `attach_at_impl`; only the compare-and-maybe-insert is under
/// it.
pub(super) fn reattach_at_impl<F>(
    inner: &Arc<Mutex<Registry>>,
    harness_id: &str,
    expected_generation: u64,
    path: PathBuf,
    on_event: F,
    actions: Option<ActionPersistence>,
) -> Result<ReattachInstall, ClaudeEventsError>
where
    F: Fn(ClaudeEvent) + Send + Sync + 'static,
{
    let (mut adapter, _info) = build_adapter(harness_id, path, on_event, actions)?;
    let mut reg = inner.lock();
    let current_generation = reg.adapters.get(harness_id).map(|a| a.generation);
    if current_generation != Some(expected_generation) {
        let still_present = current_generation.is_some();
        drop(reg);
        return Ok(ReattachInstall::Abandoned { still_present });
    }
    reg.next_generation += 1;
    adapter.generation = reg.next_generation;
    reg.adapters.insert(harness_id.to_string(), adapter);
    drop(reg);
    Ok(ReattachInstall::Installed)
}
