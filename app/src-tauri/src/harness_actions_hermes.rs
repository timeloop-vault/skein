//! Hermes `state.db` tail: the feed's action rows and per-turn cost
//! for a Hermes harness (#625, the Hermes counterpart of
//! `harness_actions_claude` / `_opencode`).
//!
//! Hermes writes every message into its profile's `state.db`, so unlike
//! Claude (JSONL tail) and opencode (SSE) there is no event to wait for:
//! one [`Tail`] per harness polls the file and turns new `messages` rows
//! into `harness_actions` rows with `skein_harness::hermes::messages`
//! (which owns the row shapes). The poll is cheap by construction — the
//! query is skipped while `state.db` and its `-wal` are unchanged.
//!
//! # No duplicates across restarts
//!
//! Every recorded row carries `message_id` and `session_id` in its
//! payload; on attach the cursor is the highest `message_id` already
//! recorded for the harness + session
//! ([`Database::max_recorded_message_id`]). Nothing recorded means 0: the
//! session is backfilled once. The messages module skips tool rows that
//! have no owning call in range, so resuming from an assistant row's id is
//! safe.
//!
//! # Per-turn cost
//!
//! Hermes keeps only a CUMULATIVE session total, so the frontend calls
//! `hermes_actions_turn_end` on a real end of turn and the tail records
//! the delta against the previous snapshot as a `turn_cost` row in the
//! opencode step shape (`tokens` + `cost` + terminal `reason`) the feed
//! already folds. The cumulative snapshot rides in the payload
//! (`session_*`), which is both the restart baseline and what makes
//! successive rows distinct to the feed's identical-payload de-dupe.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use parking_lot::Mutex;
use serde_json::{Value, json};
use skein_harness::hermes::{
    self,
    messages::{self, HermesCost},
};
use tauri::AppHandle;
use tokio::task::JoinHandle;

use crate::db::Database;

/// `harness_actions.source` of every row this tail writes.
pub(crate) const SOURCE: &str = "l2c3-hermes-statedb";
const HARNESS_KIND: &str = "hermes";
const POLL: Duration = Duration::from_millis(750);
/// Query anyway after this many unchanged polls, in case a write left
/// both size and mtime where they were.
const MAX_SKIPS: u32 = 8;
const COST_EPSILON: f64 = 1e-9;

/// (size, mtime) of `state.db` and `state.db-wal`.
type Stat = [Option<(u64, SystemTime)>; 2];

fn stat_of(db: &Path) -> Stat {
    let one = |p: &Path| {
        let m = std::fs::metadata(p).ok()?;
        Some((m.len(), m.modified().ok()?))
    };
    let mut wal = db.as_os_str().to_owned();
    wal.push("-wal");
    [one(db), one(Path::new(&wal))]
}

/// Cumulative session spend and tokens, as stored in a `turn_cost`
/// payload.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Snapshot {
    usd: f64,
    aux_usd: f64,
    input: i64,
    output: i64,
    reasoning: i64,
    cache_read: i64,
    cache_write: i64,
}

impl Snapshot {
    fn of(c: &HermesCost) -> Self {
        Self {
            usd: c.actual_cost_usd.or(c.estimated_cost_usd).unwrap_or(0.0),
            aux_usd: c.auxiliary_cost_usd,
            input: c.input_tokens,
            output: c.output_tokens,
            reasoning: c.reasoning_tokens,
            cache_read: c.cache_read_tokens,
            cache_write: c.cache_write_tokens,
        }
    }

    fn from_payload(p: &Value) -> Self {
        let t = &p["session_tokens"];
        let n = |k: &str| t[k].as_i64().unwrap_or(0);
        Self {
            usd: p["session_cost_usd"].as_f64().unwrap_or(0.0),
            aux_usd: p["session_aux_cost_usd"].as_f64().unwrap_or(0.0),
            input: n("input"),
            output: n("output"),
            reasoning: n("reasoning"),
            cache_read: n("cache_read"),
            cache_write: n("cache_write"),
        }
    }
}

/// The `turn_cost` payload for the growth from `base` to `now`, or `None`
/// when neither tokens nor cost grew. Negative components (a counter that
/// went backwards) clamp to 0.
fn turn_cost_payload(
    session_id: &str,
    model: Option<&str>,
    base: &Snapshot,
    now: &Snapshot,
) -> Option<Value> {
    let d = |a: i64, b: i64| (a - b).max(0);
    let (input, output) = (d(now.input, base.input), d(now.output, base.output));
    let reasoning = d(now.reasoning, base.reasoning);
    let (read, write) = (
        d(now.cache_read, base.cache_read),
        d(now.cache_write, base.cache_write),
    );
    let total = input + output + reasoning + read + write;
    let cost = ((now.usd - base.usd).max(0.0)) + ((now.aux_usd - base.aux_usd).max(0.0));
    if total == 0 && cost <= COST_EPSILON {
        return None;
    }
    Some(json!({
        // The opencode step shape `stepFromOpencode` reads; a `reason`
        // other than "tool-calls" ends the turn.
        "cost": cost,
        "reason": "stop",
        "tokens": {
            "total": total,
            "input": input,
            "output": output,
            "reasoning": reasoning,
            "cache": { "read": read, "write": write },
        },
        "model": model,
        "session_id": session_id,
        // The cumulative snapshot: next turn's baseline after a restart.
        "session_cost_usd": now.usd,
        "session_aux_cost_usd": now.aux_usd,
        "session_tokens": {
            "input": now.input,
            "output": now.output,
            "reasoning": now.reasoning,
            "cache_read": now.cache_read,
            "cache_write": now.cache_write,
        },
    }))
}

fn current_snapshot(state_db: &Path, session_id: &str) -> Option<Snapshot> {
    let conn = hermes::open(state_db).ok()??;
    let cost = messages::session_cost(&conn, session_id).ok()??;
    Some(Snapshot::of(&cost))
}

struct Inner {
    cursor: i64,
    baseline: Snapshot,
    last_stat: Option<Stat>,
    skips: u32,
    warned: bool,
    /// Set once a poll has read the table: whatever that poll found is
    /// history that predates this attach, never live.
    replayed: bool,
}

/// One harness's tail. All state lives behind one lock so the poll and
/// `turn_end` can never record the same row twice.
pub(crate) struct Tail {
    db: Arc<Database>,
    /// `None` in tests.
    app: Option<AppHandle>,
    harness_id: String,
    room_id: String,
    session_id: String,
    state_db: PathBuf,
    stopped: AtomicBool,
    inner: Mutex<Inner>,
}

impl Tail {
    pub(crate) fn new(
        db: Arc<Database>,
        app: Option<AppHandle>,
        harness_id: String,
        room_id: String,
        session_id: String,
        state_db: PathBuf,
        resumed: bool,
    ) -> Self {
        let cursor = db
            .max_recorded_message_id(&harness_id, &session_id, SOURCE)
            .unwrap_or_else(|e| {
                tracing::warn!(%harness_id, error = %e, "hermes tail: cursor recovery failed");
                None
            })
            .unwrap_or(0);
        let recorded = db
            .last_turn_cost_payload(&harness_id, &session_id, SOURCE)
            .ok()
            .flatten()
            .and_then(|p| serde_json::from_str::<Value>(&p).ok())
            .map(|p| Snapshot::from_payload(&p));
        // A resumed session already has spend that is not this turn's:
        // with no recorded baseline, start from what it is now. Residual
        // gap: spend between this attach and a restart before the FIRST
        // turn_end is dropped (the next resumed attach re-snapshots). A
        // fresh session (not resumed) starts from zero.
        let baseline = recorded.unwrap_or_else(|| {
            if resumed {
                current_snapshot(&state_db, &session_id).unwrap_or_default()
            } else {
                Snapshot::default()
            }
        });
        Self {
            db,
            app,
            harness_id,
            room_id,
            session_id,
            state_db,
            stopped: AtomicBool::new(false),
            inner: Mutex::new(Inner {
                cursor,
                baseline,
                last_stat: None,
                skips: 0,
                warned: false,
                replayed: false,
            }),
        }
    }

    /// One poll: record whatever is new. Blocking.
    pub(crate) fn poll(&self) {
        let mut inner = self.inner.lock();
        self.flush(&mut inner, false);
    }

    fn warn_once(&self, inner: &mut Inner, what: &str, e: &dyn std::fmt::Display) {
        if !inner.warned {
            inner.warned = true;
            tracing::warn!(harness_id = %self.harness_id, error = %e, "hermes tail: {what}; will keep polling");
        }
    }

    /// `live`: the row was discovered after attach. Only live rows are
    /// broadcast and (for `patch`) capture a review baseline; replayed
    /// history would read a HEAD that has already moved past the edit.
    fn record(
        &self,
        timestamp_ms: i64,
        kind: &str,
        payload: &Value,
        live: bool,
    ) -> Result<i64, String> {
        let text = payload.to_string();
        let id = self.db.record_harness_action(
            &self.harness_id,
            &self.room_id,
            timestamp_ms,
            kind,
            &text,
            Some(SOURCE),
            Some(HARNESS_KIND),
        )?;
        if !live {
            return Ok(id);
        }
        if kind == crate::db::action_kind::PATCH {
            self.note_patch(&text);
        }
        if let Some(app) = &self.app {
            crate::harness_action_event::emit(
                app,
                id,
                &self.harness_id,
                &self.room_id,
                timestamp_ms,
                kind,
                &text,
                Some(SOURCE),
                Some(HARNESS_KIND),
            );
        }
        Ok(id)
    }

    fn note_patch(&self, payload: &str) {
        match self.db.room_by_id(&self.room_id) {
            Ok(Some(room)) => {
                if let Some(cwd) = room.cwd.as_deref() {
                    crate::review::note_patch(
                        &self.db,
                        &self.room_id,
                        cwd,
                        &self.harness_id,
                        payload,
                    );
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(harness_id = %self.harness_id, error = %e, "hermes tail: room lookup failed");
            }
        }
    }

    /// Record the messages after the cursor. `force` skips the
    /// unchanged-file shortcut.
    fn flush(&self, inner: &mut Inner, force: bool) {
        if self.stopped.load(Ordering::Relaxed) {
            return;
        }
        let stat = stat_of(&self.state_db);
        if !force && inner.last_stat == Some(stat) && inner.skips < MAX_SKIPS {
            inner.skips += 1;
            return;
        }
        inner.skips = 0;
        let conn = match hermes::open(&self.state_db) {
            Ok(Some(c)) => c,
            Ok(None) => return, // Hermes has not created it yet.
            Err(e) => return self.warn_once(inner, "opening state.db failed", &e),
        };
        let (rows, cursor) = match messages::actions_after(&conn, &self.session_id, inner.cursor) {
            Ok(r) => r,
            Err(e) => return self.warn_once(inner, "reading messages failed", &e),
        };
        inner.warned = false;
        let live = inner.replayed;
        inner.replayed = true;
        let mut done_through = inner.cursor;
        let mut prev_id: Option<i64> = None;
        for mut row in rows {
            // Rows of one message share its id: only once a message is
            // fully recorded may the cursor pass it.
            if let Some(p) = prev_id
                && p != row.message_id
            {
                done_through = p;
            }
            prev_id = Some(row.message_id);
            if let Some(o) = row.payload.as_object_mut() {
                o.insert("message_id".into(), json!(row.message_id));
                o.insert("session_id".into(), json!(self.session_id));
            }
            if let Err(e) = self.record(row.timestamp_ms, &row.kind, &row.payload, live) {
                // The cursor stops after the last fully recorded message,
                // so the rest is retried without repeating those rows.
                inner.cursor = done_through;
                tracing::warn!(harness_id = %self.harness_id, error = %e, "hermes tail: recording failed");
                return;
            }
        }
        inner.cursor = cursor;
        inner.last_stat = Some(stat);
    }

    /// A real end of turn: flush the messages first (so the turn's tool
    /// rows precede its cost row), then record the cost growth since the
    /// last snapshot. `Ok(true)` when a `turn_cost` row was recorded.
    pub(crate) fn turn_end(&self) -> Result<bool, String> {
        let mut inner = self.inner.lock();
        if self.stopped.load(Ordering::Relaxed) {
            return Ok(false);
        }
        self.flush(&mut inner, true);
        let Some(conn) = hermes::open(&self.state_db).map_err(|e| e.to_string())? else {
            return Ok(false);
        };
        let Some(cost) =
            messages::session_cost(&conn, &self.session_id).map_err(|e| e.to_string())?
        else {
            return Ok(false);
        };
        let now = Snapshot::of(&cost);
        let payload = turn_cost_payload(
            &self.session_id,
            cost.model.as_deref(),
            &inner.baseline,
            &now,
        );
        let grew = payload.is_some();
        if let Some(p) = payload {
            self.record(crate::review::now_ms(), "turn_cost", &p, true)?;
        }
        // A counter that went backwards (a reset) also moves the baseline.
        inner.baseline = now;
        Ok(grew)
    }
}

struct Entry {
    tail: Arc<Tail>,
    session_id: String,
    state_db: PathBuf,
    handle: JoinHandle<()>,
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.tail.stopped.store(true, Ordering::Relaxed);
        self.handle.abort();
    }
}

/// The tails, keyed by harness id.
pub struct HermesActionsManager {
    inner: Mutex<HashMap<String, Entry>>,
    db: Arc<Database>,
    app: AppHandle,
}

impl HermesActionsManager {
    pub fn new(db: Arc<Database>, app: AppHandle) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            db,
            app,
        }
    }

    /// Must run inside the tokio runtime (it spawns the poll task). The
    /// tail's initial db scans run on the blocking pool, before the
    /// manager lock is taken.
    async fn attach(
        &self,
        harness_id: String,
        room_id: String,
        session_id: String,
        state_db: PathBuf,
        resumed: bool,
    ) -> Result<(), String> {
        {
            let inner = self.inner.lock();
            if let Some(e) = inner.get(&harness_id)
                && e.session_id == session_id
                && e.state_db == state_db
            {
                return Ok(());
            }
        }
        let tail = {
            let (db, app) = (Arc::clone(&self.db), self.app.clone());
            let (h, s, p) = (harness_id.clone(), session_id.clone(), state_db.clone());
            tokio::task::spawn_blocking(move || {
                Arc::new(Tail::new(db, Some(app), h, room_id, s, p, resumed))
            })
            .await
            .map_err(|e| e.to_string())?
        };
        let mut inner = self.inner.lock();
        inner.remove(&harness_id); // Drop stops the old tail.
        let t = Arc::clone(&tail);
        let handle = tokio::spawn(async move {
            let mut tick = tokio::time::interval(POLL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let t = Arc::clone(&t);
                if tokio::task::spawn_blocking(move || t.poll()).await.is_err() {
                    break; // The poll panicked; do not spin on it.
                }
            }
        });
        inner.insert(
            harness_id,
            Entry {
                tail,
                session_id,
                state_db,
                handle,
            },
        );
        Ok(())
    }

    fn detach(&self, harness_id: &str) {
        self.inner.lock().remove(harness_id);
    }

    fn tail(&self, harness_id: &str) -> Option<Arc<Tail>> {
        self.inner
            .lock()
            .get(harness_id)
            .map(|e| Arc::clone(&e.tail))
    }
}

/// Start (or replace) the state.db tail for a Hermes harness. The same
/// session again is a no-op; a different one replaces the tail.
/// `resumed`: the session already had spend, so per-turn cost starts from
/// its current totals instead of zero (unless a baseline was recorded).
#[tauri::command]
pub(crate) async fn hermes_actions_attach(
    harness_id: String,
    room_id: String,
    session_id: String,
    hermes_profile: Option<String>,
    resumed: Option<bool>,
    manager: tauri::State<'_, HermesActionsManager>,
    spawn_env: tauri::State<'_, crate::commands::spawn_env::SpawnEnvState>,
) -> Result<(), String> {
    let settings = spawn_env.snapshot();
    let state_db = crate::resume::profile_db(&settings, hermes_profile.as_deref())
        .ok_or_else(|| "no Hermes home or invalid profile".to_string())?;
    manager
        .attach(
            harness_id,
            room_id,
            session_id,
            state_db,
            resumed.unwrap_or(false),
        )
        .await
}

/// Stop the tail for `harness_id`. No-op if unknown.
#[allow(clippy::needless_pass_by_value)]
#[tauri::command]
pub(crate) fn hermes_actions_detach(
    harness_id: String,
    manager: tauri::State<'_, HermesActionsManager>,
) {
    manager.detach(&harness_id);
}

/// A real end of turn: record the turn's cost. `true` when a `turn_cost`
/// row was recorded; `false` for no tail, no session row or no growth.
#[tauri::command]
pub(crate) async fn hermes_actions_turn_end(
    harness_id: String,
    manager: tauri::State<'_, HermesActionsManager>,
) -> Result<bool, String> {
    let Some(tail) = manager.tail(&harness_id) else {
        return Ok(false);
    };
    tokio::task::spawn_blocking(move || tail.turn_end())
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests;
