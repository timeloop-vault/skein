//! Streams a Hermes harness's outstanding work (#628): background
//! `delegate_task` children and background terminal processes, read from
//! the profile's `state.db` and `processes.json` by
//! `skein_harness::hermes::work`. The frontend (`hermesWork.ts`) feeds
//! these into the same end-of-turn deferral Claude's subagents use.
//!
//! Polled, not watched: `state.db` is WAL, so a watcher would need the
//! `-wal` sidecar and `processes.json` too, for a 1 s granularity gain
//! nothing here needs. Disk is the source of truth, so each pass is a
//! full re-derivation diffed against the last one; the first pass is the
//! attach snapshot (`initial: true`). `delegationsIdle` is the one edge
//! event: the live-owner delegation count going from >=1 to 0 between two
//! successful passes (never on the first pass; a read error keeps the
//! previous state). Emitted between the ends and the starts.
//!
//! Liveness is a plain "pid exists" check via sysinfo (the crate forbids
//! `unsafe`). `owner_pid` is the gateway child of the TUI, and the attach
//! arguments do not name the PTY, so a descendant check (which would also
//! defeat pid reuse) is not possible without widening the command.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;
use skein_harness::hermes::work::{
    live_delegation_count, notify_process_ids_after, read_processes, running_delegations,
};
use sysinfo::{Pid, ProcessesToUpdate, System};
use tauri::ipc::Channel;
use tokio::task::JoinHandle;

const POLL: Duration = Duration::from_secs(1);

/// What the frontend receives; mirrored by `hermesWork.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum HermesWorkEvent {
    #[serde(rename_all = "camelCase")]
    SubagentStart {
        agent_id: String,
        description: Option<String>,
        initial: bool,
    },
    #[serde(rename_all = "camelCase")]
    SubagentEnd { agent_id: String },
    #[serde(rename_all = "camelCase")]
    BackgroundStart {
        task_id: String,
        command: Option<String>,
        initial: bool,
    },
    #[serde(rename_all = "camelCase")]
    BackgroundEnd { task_id: String },
    /// The last running delegation with a live owner is gone. Independent
    /// of the child-linking heuristic, so the frontend can release
    /// hook-started subagents whose `subagent_stop` was lost.
    DelegationsIdle,
}

/// The outstanding work at one instant: id → description / command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Work {
    pub subagents: BTreeMap<String, Option<String>>,
    pub tasks: BTreeMap<String, Option<String>>,
    /// Running delegation rows with a live owner (not heuristic-linked).
    pub live_delegations: usize,
}

/// `delegationsIdle` is an edge: the live count was at least 1 on the
/// previous successful pass (`None` on the first, attach pass) and is 0 now.
pub(crate) fn idle_edge(prev: Option<usize>, next: usize) -> bool {
    next == 0 && prev.is_some_and(|p| p >= 1)
}

/// Ends for what vanished, then `delegationsIdle` if `idle`, then starts
/// for what appeared. `initial` marks the starts of the attach snapshot.
pub(crate) fn diff(prev: &Work, next: &Work, initial: bool, idle: bool) -> Vec<HermesWorkEvent> {
    let mut out = Vec::new();
    for id in prev
        .subagents
        .keys()
        .filter(|k| !next.subagents.contains_key(*k))
    {
        out.push(HermesWorkEvent::SubagentEnd {
            agent_id: id.clone(),
        });
    }
    for id in prev.tasks.keys().filter(|k| !next.tasks.contains_key(*k)) {
        out.push(HermesWorkEvent::BackgroundEnd {
            task_id: id.clone(),
        });
    }
    if idle {
        out.push(HermesWorkEvent::DelegationsIdle);
    }
    for (id, d) in next
        .subagents
        .iter()
        .filter(|(k, _)| !prev.subagents.contains_key(*k))
    {
        out.push(HermesWorkEvent::SubagentStart {
            agent_id: id.clone(),
            description: d.clone(),
            initial,
        });
    }
    for (id, c) in next
        .tasks
        .iter()
        .filter(|(k, _)| !prev.tasks.contains_key(*k))
    {
        out.push(HermesWorkEvent::BackgroundStart {
            task_id: id.clone(),
            command: c.clone(),
            initial,
        });
    }
    out
}

/// Whether a process with this pid exists right now.
fn pid_exists(pid: u32) -> bool {
    let p = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[p]), true);
    sys.process(p).is_some()
}

/// Read the live set from disk. A missing `state.db` is "no delegations";
/// `processes.json` lives beside it.
pub(crate) fn read_work(
    state_db: &Path,
    session_id: &str,
    pid_alive: impl Fn(u32) -> bool,
    notify: &mut NotifyState,
) -> Result<Work, String> {
    let mut work = Work::default();
    let processes = state_db
        .parent()
        .map(|dir| read_processes(&dir.join("processes.json"), session_id))
        .unwrap_or_default();
    if let Some(conn) = skein_harness::hermes::open(state_db).map_err(|e| e.to_string())? {
        for d in running_delegations(&conn, session_id, &pid_alive).map_err(|e| e.to_string())? {
            work.subagents.insert(d.child_session_id, d.description);
        }
        work.live_delegations =
            live_delegation_count(&conn, session_id, &pid_alive).map_err(|e| e.to_string())?;
        // Skipped with no process to classify; the cursor then stays put and
        // the next pass that needs it catches up.
        if !processes.is_empty() {
            let (ids, cursor) = notify_process_ids_after(&conn, session_id, notify.cursor)
                .map_err(|e| e.to_string())?;
            notify.ids.extend(ids);
            if let Some(c) = cursor {
                notify.cursor = c;
            }
        }
    }
    for p in processes {
        // Only a process that wakes the session counts (the file's own
        // flag is stale, see the crate module docs); a pid known to be
        // gone is a stale entry.
        if notify.ids.contains(&p.id) && p.pid.is_none_or(&pid_alive) {
            work.tasks.insert(p.id, p.command);
        }
    }
    Ok(work)
}

/// What one poller has learned from `messages`: notify process ids so far
/// and the highest `messages.id` already scanned.
#[derive(Debug, Default)]
pub(crate) struct NotifyState {
    ids: HashSet<String>,
    cursor: i64,
}

/// One poller's state; `step` is a single poll pass.
struct Poller {
    state_db: PathBuf,
    session_id: String,
    channel: Channel<HermesWorkEvent>,
    prev: Work,
    notify: NotifyState,
    seeded: bool,
    warned: bool,
}

impl Poller {
    /// `false` once the channel is gone and polling should stop.
    fn step(&mut self, pid_alive: impl Fn(u32) -> bool) -> bool {
        let next = match read_work(
            &self.state_db,
            &self.session_id,
            pid_alive,
            &mut self.notify,
        ) {
            Ok(w) => {
                self.warned = false;
                w
            }
            Err(e) => {
                if !self.warned {
                    self.warned = true;
                    tracing::warn!(error = %e, "hermes work: read failed; keeping last state");
                }
                return true;
            }
        };
        let prev_live = self.seeded.then_some(self.prev.live_delegations);
        let idle = idle_edge(prev_live, next.live_delegations);
        for ev in diff(&self.prev, &next, !self.seeded, idle) {
            if self.channel.send(ev).is_err() {
                return false;
            }
        }
        self.prev = next;
        self.seeded = true;
        true
    }
}

struct Entry {
    stopped: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.handle.abort();
    }
}

/// The pollers, keyed by harness id.
#[derive(Default)]
pub struct HermesWorkManager {
    inner: Mutex<HashMap<String, Entry>>,
}

impl HermesWorkManager {
    /// Must run inside the tokio runtime. Replaces any previous poller
    /// for the harness (its channel is new, so it must re-snapshot).
    fn attach(
        &self,
        harness_id: String,
        session_id: String,
        state_db: PathBuf,
        channel: Channel<HermesWorkEvent>,
    ) {
        let stopped = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stopped);
        let poller = Arc::new(Mutex::new(Poller {
            state_db,
            session_id,
            channel,
            prev: Work::default(),
            notify: NotifyState::default(),
            seeded: false,
            warned: false,
        }));
        let handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(POLL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                if flag.load(Ordering::Relaxed) {
                    break;
                }
                let p = Arc::clone(&poller);
                match tokio::task::spawn_blocking(move || p.lock().step(pid_exists)).await {
                    Ok(true) => {}
                    _ => break, // Channel closed or the pass panicked.
                }
            }
        });
        // Drop of the replaced entry stops the old poller.
        self.inner
            .lock()
            .insert(harness_id, Entry { stopped, handle });
    }

    fn detach(&self, harness_id: &str) {
        self.inner.lock().remove(harness_id);
    }
}

/// Start (or replace) the work poller for a Hermes harness; the attach
/// snapshot arrives as `initial: true` starts on the first tick.
#[tauri::command]
#[allow(clippy::needless_pass_by_value, clippy::too_many_arguments)]
pub(crate) async fn hermes_work_attach(
    harness_id: String,
    room_id: String,
    session_id: String,
    hermes_profile: Option<String>,
    resumed: Option<bool>,
    on_event: Channel<HermesWorkEvent>,
    manager: tauri::State<'_, HermesWorkManager>,
    spawn_env: tauri::State<'_, crate::commands::spawn_env::SpawnEnvState>,
) -> Result<(), String> {
    let _ = (room_id, resumed); // Same arguments as hermes_actions_attach.
    let settings = spawn_env.snapshot();
    let state_db = crate::resume::profile_db(&settings, hermes_profile.as_deref())
        .ok_or_else(|| "no Hermes home or invalid profile".to_string())?;
    manager.attach(harness_id, session_id, state_db, on_event);
    Ok(())
}

/// Stop the poller for `harness_id`. No-op if unknown.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn hermes_work_detach(harness_id: String, manager: tauri::State<'_, HermesWorkManager>) {
    manager.detach(&harness_id);
}

#[cfg(test)]
mod tests;
