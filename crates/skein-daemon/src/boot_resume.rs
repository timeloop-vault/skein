//! Boot preparation, owned by the daemon (#731, reduced by #753): at the first
//! rooms load that returns rooms, three small steps over the stored rows,
//! written through the `RoomStore` write path as field-level edits under the
//! store lock (#755: `plan.rs`; only `launch`, `shell_claim` and `cmd`, and
//! only where the record still matches what the plan was computed from, so a
//! client put or the runner's session-id write-back made since the snapshot is
//! never overwritten, and a harness with a process or a spawn in flight takes
//! just the `launch` migration). The launch migration is applied whenever the
//! stored `launch`, `cmd` and `shell_claim` still equal the snapshot's, even
//! for a stale or busy harness; the other two fields need the record to match
//! in full. Nothing is spawned here; that
//! happens on first request, and `ensure_running` derives the argv from the
//! record then (#752), so resume-versus-fresh, stale session ids and session
//! minting are the spawn's business, not this module's.
//!
//! 1. `migrate_launch`: fill in `Harness.launch` on pre-#752 records.
//! 2. The #517 shell claim: a live claim turns a shell-mode Claude/opencode
//!    harness back into its tool (`launch` and claim cleared, `cmd` reset to
//!    the bare program); a stale one is just cleared.
//! 3. A fresh opencode port: the spawn re-passes the `--port` parsed from the
//!    stored cmd, so each process start needs a new free one in it.
//!
//! The preparation is a barrier (#744). A mutex is held across the load and
//! the preparation, so a load that starts meanwhile (a reload, a double
//! hydrate, the retry button) waits, then does its own load and reads the
//! prepared rows. No load returns while the preparation is still running; a
//! caller gets unprepared rows only when the preparation itself failed, and
//! then the next load tries again.
//! Lock order: this barrier is taken first, before the store lock and the db
//! lock, and never while holding either; then the store lock, then, inside
//! the edit, the runner's `entries`/gate map briefly via `is_busy`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use skein_proto::harness_kind::HarnessKind;
use skein_proto::rooms::{Harness, Room};

use crate::db::{Database, LoadOutcome};
use crate::harness_argv::{is_opencode_v2, migrate_launch, opencode_port_args, shell_argv};
use crate::pty::opencode::pick_free_port;
use crate::room_store::RoomStore;
use crate::spawn_env::state::SpawnEnvState;

pub mod plan;
use plan::{BootPlan, Counts};

/// The `origin` the preparation's `rooms-changed` event carries.
pub const BOOT_ORIGIN: &str = "daemon-boot";

/// Once-per-process state of the boot preparation.
pub struct BootResume {
    /// The barrier. `true` once a load with rooms has been through a
    /// preparation that succeeded.
    done: Mutex<bool>,
    /// `None` only in tests; Hermes then reads as "can't tell".
    spawn_env: Option<Arc<SpawnEnvState>>,
    /// Does the harness have a process, or one starting, right now? A retried
    /// preparation must not rewrite a harness that is up.
    busy: Arc<dyn Fn(&str) -> bool + Send + Sync>,
}

/// The `--port <p>` of an opencode argv: how a later step recovers the port
/// this module allocated, without a side map.
#[must_use]
pub fn opencode_port_from_cmd(cmd: &[String]) -> Option<u16> {
    let at = cmd.iter().position(|a| a == "--port")?;
    cmd.get(at + 1)?.parse().ok()
}

/// The real "does this harness's session still exist" probe, shared by the
/// boot check and the runner's spawn-time derivation (#752). Runs on a
/// blocking thread, so the async probes are driven with the ambient runtime
/// handle; with none, the probe errors (and the caller keeps the id). Hermes
/// without a `spawn_env` reads as "can't tell" (`Ok(true)`).
///
/// # Errors
/// No ambient runtime, or the underlying probe failed.
pub fn probe_session(spawn_env: Option<&SpawnEnvState>, h: &Harness) -> Result<bool, String> {
    let sid = h.session_id.clone().unwrap_or_default();
    let handle = tokio::runtime::Handle::try_current().map_err(|e| e.to_string())?;
    match HarnessKind::from_name(&h.kind) {
        Some(HarnessKind::Claude) => handle.block_on(crate::resume::claude_session_exists(sid)),
        Some(HarnessKind::Opencode) => handle.block_on(crate::resume::opencode_session_exists(sid)),
        Some(HarnessKind::Hermes) => {
            let Some(env) = spawn_env else {
                return Ok(true);
            };
            let settings = env.snapshot();
            let profile = h.hermes_profile.clone();
            let resolve = move || -> Option<PathBuf> {
                crate::resume::profile_db(&settings, profile.as_deref())
            };
            handle.block_on(crate::resume::hermes_session_exists(resolve, sid))
        }
        _ => Ok(true),
    }
}

/// Does the harness's stored session id survive the boot check? `probe`
/// answers "does this session still exist"; an error keeps the id.
pub fn session_survives(h: &Harness, probe: &impl Fn(&Harness) -> Result<bool, String>) -> bool {
    let Some(sid) = h.session_id.as_deref() else {
        return true;
    };
    let kind = HarnessKind::from_name(&h.kind);
    if kind == Some(HarnessKind::Opencode) && is_opencode_v2(h.opencode_version.as_ref()) {
        // #679: the v1 table cannot see v2 sessions; trust the stored id.
        return !sid.trim().is_empty();
    }
    if !matches!(
        kind,
        Some(HarnessKind::Claude | HarnessKind::Opencode | HarnessKind::Hermes)
    ) {
        return true;
    }
    match probe(h) {
        Ok(exists) => exists,
        Err(e) => {
            tracing::warn!(kind = %h.kind, error = %e, "boot resume: session probe failed; keeping the id");
            true
        }
    }
}

/// The three boot steps over the stored rooms, with the probe, port picker
/// and busy check injected. `session_id` is read, never changed. The busy
/// check here only avoids picking a port for a harness that is up; the one
/// under the store lock decides (see `prepare`).
///
/// # Errors
/// The port picker failed; nothing is returned then.
pub fn prepare_with(
    mut rooms: Vec<Room>,
    probe: impl Fn(&Harness) -> Result<bool, String>,
    mut pick: impl FnMut() -> Result<u16, String>,
    is_busy: impl Fn(&str) -> bool,
) -> Result<Vec<Room>, String> {
    for room in &mut rooms {
        // #752's one-time migration: records written before `launch` existed
        // get it filled in here, and the plan persists it (for a busy harness
        // too). It must run once per boot, before any spawn.
        migrate_launch(room);
        let archived = room.archived.is_some();
        for h in &mut room.harnesses {
            if h.cmd.is_none() || is_busy(&h.id) {
                continue;
            }
            let Some(kind) = HarnessKind::from_name(&h.kind) else {
                continue;
            };
            if matches!(kind, HarnessKind::Claude | HarnessKind::Opencode) {
                settle_shell_claim(h, kind, &probe);
            }
            if kind == HarnessKind::Opencode && !archived && shell_argv(h).is_none() {
                let port = pick()?;
                let mut cmd = vec!["opencode".to_owned()];
                cmd.extend(opencode_port_args(port));
                h.cmd = Some(cmd);
            }
        }
    }
    Ok(rooms)
}

/// The #517 claim: live (same non-blank session, which still exists) puts a
/// shell-mode harness back on its tool; anything else just drops the claim.
/// The session id is never touched; the spawn drops a stale one itself. The
/// probe is only called for a shell-mode harness whose claim matches.
fn settle_shell_claim(
    h: &mut Harness,
    kind: HarnessKind,
    probe: &impl Fn(&Harness) -> Result<bool, String>,
) {
    let Some(claim) = h.shell_claim.as_ref() else {
        return;
    };
    if shell_argv(h).is_none() {
        h.shell_claim = None;
        return;
    }
    let matches =
        !claim.session_id.trim().is_empty() && h.session_id.as_deref() == Some(&claim.session_id);
    let live = matches && session_survives(h, probe);
    h.shell_claim = None;
    if live && let Some(program) = kind.program() {
        h.launch = None;
        h.cmd = Some(vec![program.to_owned()]);
    }
}

impl BootResume {
    /// `busy` answers "does this harness id have a process, or one starting,
    /// right now". It is called under the store lock, so it must not block.
    #[must_use]
    pub fn new(
        spawn_env: Arc<SpawnEnvState>,
        busy: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    ) -> Self {
        Self {
            done: Mutex::new(false),
            spawn_env: Some(spawn_env),
            busy,
        }
    }

    #[cfg(test)]
    fn detached() -> Self {
        Self {
            done: Mutex::new(false),
            spawn_env: None,
            busy: Arc::new(|_| false),
        }
    }

    /// The real probe. Runs on a blocking thread (the load is), so the async
    /// probes are driven with the ambient runtime handle; with none, the
    /// probe errors and the id is kept.
    fn probe(&self, h: &Harness) -> Result<bool, String> {
        probe_session(self.spawn_env.as_deref(), h)
    }

    /// [`crate::rooms::load_rooms`], then, until a preparation has
    /// succeeded, the boot preparation. The prepared rooms are what the store
    /// holds and what the outcome returns. A load that starts while the
    /// preparation runs waits for it and then reads the prepared rows; so
    /// does every later load. A failed load does not use up the preparation,
    /// and neither does a load with no rooms. A preparation failure is logged
    /// and the unprepared rooms are returned; the next load tries again. A
    /// retry can run while harnesses are up, so the running check is what
    /// keeps it from touching a live harness.
    ///
    /// # Errors
    /// The underlying load failed.
    pub fn load_rooms(&self, db: &Arc<Database>, store: &RoomStore) -> Result<LoadOutcome, String> {
        self.load_rooms_with(db, store, |h| self.probe(h), pick_free_port)
    }

    fn load_rooms_with(
        &self,
        db: &Arc<Database>,
        store: &RoomStore,
        probe: impl Fn(&Harness) -> Result<bool, String>,
        pick: impl FnMut() -> Result<u16, String>,
    ) -> Result<LoadOutcome, String> {
        // Held to the end: see the module doc for the lock order. A poisoned
        // guard is recovered, since the flag is always valid.
        let mut done = self.done.lock().unwrap_or_else(PoisonError::into_inner);
        let mut outcome = crate::rooms::load_rooms(db, store)?;
        if !*done && !outcome.rooms.is_empty() {
            match prepare(store, &mut outcome, probe, pick, &*self.busy) {
                Ok(()) => *done = true,
                Err(e) => {
                    tracing::error!(error = %e, "boot resume: preparation failed; rooms left unprepared");
                }
            }
        }
        Ok(outcome)
    }
}

fn prepare(
    store: &RoomStore,
    outcome: &mut LoadOutcome,
    probe: impl Fn(&Harness) -> Result<bool, String>,
    pick: impl FnMut() -> Result<u16, String>,
    is_busy: impl Fn(&str) -> bool,
) -> Result<(), String> {
    let prepared = prepare_with(outcome.rooms.clone(), probe, pick, &is_busy)?;
    let plan = BootPlan::new(&outcome.rooms, &prepared);
    let ports_refreshed = outcome
        .rooms
        .iter()
        .zip(&prepared)
        .flat_map(|(b, a)| b.harnesses.iter().zip(&a.harnesses))
        .filter(|(hb, ha)| {
            hb.cmd != ha.cmd && ha.cmd.as_deref().and_then(opencode_port_from_cmd).is_some()
        })
        .count();
    if plan.is_empty() {
        tracing::info!(
            rooms = outcome.rooms.len(),
            harnesses_changed = 0,
            ports_refreshed,
            "boot resume: prepared the stored rooms"
        );
        return Ok(());
    }
    // One field-level edit under the store lock (#755): the stale and busy
    // checks run on the record as stored now. Counts are logged after.
    let mut counts = Counts::default();
    store.edit_harnesses(BOOT_ORIGIN, |_room_id, h| {
        if let Some(p) = plan.get(&h.id) {
            let busy = is_busy(&h.id);
            counts.record(p.apply(h, busy));
        }
    })?;
    tracing::info!(
        rooms = outcome.rooms.len(),
        harnesses_changed = plan.len(),
        applied = counts.applied,
        stale = counts.stale,
        busy = counts.busy,
        ports_refreshed,
        "boot resume: prepared the stored rooms"
    );
    // The store holds the prepared rooms from here on, so that is what the
    // client's baseline must be, whether or not the write below succeeds.
    outcome.rooms = store.rooms();
    // Non-structural changes are debounced; the cmds are read by the spawn
    // path, so write them now. A failure is logged: the store is still right
    // and the debounced write retries.
    if let Err(e) = store.flush() {
        tracing::error!(error = %e, "boot resume: flushing the prepared rooms failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
