//! Boot resume, owned by the daemon (#731): once per daemon process, at the
//! first successful rooms load, probe each harness's stored session, drop the
//! stale ids, rewrite every harness `cmd` to its resume form (allocating fresh
//! opencode ports) and store the result through the `RoomStore` write path.
//! Nothing is spawned here; that happens on first request.
//!
//! The preparation is a barrier (#744). A mutex is held across the load and
//! the preparation, so a load that starts meanwhile (a reload, a double
//! hydrate, the retry button) waits, then does its own load and reads the
//! prepared rows. No load returns while the preparation is still running; a
//! caller gets unprepared rows only when the preparation itself failed.
//! Lock order: this barrier is taken first, before the store lock and the db
//! lock, and never while holding either.
//!
//! This is the port of the frontend's hydrate step (`useRoomsHydrate.ts` +
//! `roomsStoreProbe.ts` `stillExists`): a session id is stale only when its
//! probe says `false`; a probe error keeps the id; opencode v2 skips the
//! probe. Archived rooms are included, exactly as the frontend did.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use skein_proto::harness_kind::HarnessKind;
use skein_proto::room_mutations::RoomMutation;
use skein_proto::rooms::{Harness, Room};

use crate::db::{Database, LoadOutcome};
use crate::harness_argv::{
    ResumeRoomsInput, ResumeRoomsOutput, is_opencode_v2, resume_rooms_with, uuid_session_id,
};
use crate::pty::opencode::pick_free_port;
use crate::room_store::RoomStore;
use crate::spawn_env::state::SpawnEnvState;

/// The `origin` the preparation's `rooms-changed` event carries.
pub const BOOT_ORIGIN: &str = "daemon-boot";

/// Once-per-process state of the boot preparation.
pub struct BootResume {
    /// The barrier. `true` once the first successful load has been through
    /// the preparation (or skipped it because there were no rooms).
    done: Mutex<bool>,
    /// `None` only in tests; Hermes then reads as "can't tell".
    spawn_env: Option<Arc<SpawnEnvState>>,
}

/// The `--port <p>` of an opencode resume argv: how a later step recovers the
/// port this module allocated, without a side map.
#[must_use]
pub fn opencode_port_from_cmd(cmd: &[String]) -> Option<u16> {
    let at = cmd.iter().position(|a| a == "--port")?;
    cmd.get(at + 1)?.parse().ok()
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

/// Drop stale session ids, then rewrite every harness to resume form, with
/// the probe, port picker and session minter injected.
pub fn prepare_with(
    mut rooms: Vec<Room>,
    probe: impl Fn(&Harness) -> Result<bool, String>,
    pick: impl FnMut() -> Result<u16, String>,
    mint: impl FnMut() -> String,
) -> ResumeRoomsOutput {
    for room in &mut rooms {
        for h in &mut room.harnesses {
            if !session_survives(h, &probe) {
                tracing::info!(kind = %h.kind, harness = %h.id, "boot resume: dropping stale sessionId");
                h.session_id = None;
            }
        }
    }
    resume_rooms_with(
        ResumeRoomsInput {
            rooms,
            unarchive: false,
        },
        pick,
        mint,
    )
}

/// One `PutHarness` per harness that differs between `before` and `after`
/// (same rooms, same harness order: resume never adds or removes either).
#[must_use]
pub fn changed_harnesses(before: &[Room], after: &[Room]) -> Vec<RoomMutation> {
    let same =
        |a: &Harness, b: &Harness| serde_json::to_value(a).ok() == serde_json::to_value(b).ok();
    let mut out = Vec::new();
    for (b, a) in before.iter().zip(after) {
        for (hb, ha) in b.harnesses.iter().zip(&a.harnesses) {
            if !same(hb, ha) {
                out.push(RoomMutation::PutHarness {
                    room_id: a.id.clone(),
                    harness: ha.clone(),
                });
            }
        }
    }
    out
}

impl BootResume {
    #[must_use]
    pub fn new(spawn_env: Arc<SpawnEnvState>) -> Self {
        Self {
            done: Mutex::new(false),
            spawn_env: Some(spawn_env),
        }
    }

    #[cfg(test)]
    fn detached() -> Self {
        Self {
            done: Mutex::new(false),
            spawn_env: None,
        }
    }

    /// The real probe. Runs on a blocking thread (the load is), so the async
    /// probes are driven with the ambient runtime handle; with none, the
    /// probe errors and the id is kept.
    fn probe(&self, h: &Harness) -> Result<bool, String> {
        let sid = h.session_id.clone().unwrap_or_default();
        let handle = tokio::runtime::Handle::try_current().map_err(|e| e.to_string())?;
        match HarnessKind::from_name(&h.kind) {
            Some(HarnessKind::Claude) => handle.block_on(crate::resume::claude_session_exists(sid)),
            Some(HarnessKind::Opencode) => {
                handle.block_on(crate::resume::opencode_session_exists(sid))
            }
            Some(HarnessKind::Hermes) => {
                let Some(env) = &self.spawn_env else {
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

    /// [`crate::rooms::load_rooms`], then, on the first successful load of
    /// this process, the boot preparation. The prepared rooms are what the
    /// store holds and what the outcome returns. A load that starts while the
    /// preparation runs waits for it and then reads the prepared rows; so
    /// does every later load. A failed load does not use up the preparation.
    /// A preparation failure is logged and the unprepared rooms are
    /// returned; it is not retried, since a later load may find the
    /// harnesses already running.
    ///
    /// # Errors
    /// The underlying load failed.
    pub fn load_rooms(&self, db: &Arc<Database>, store: &RoomStore) -> Result<LoadOutcome, String> {
        self.load_rooms_with(
            db,
            store,
            |h| self.probe(h),
            pick_free_port,
            uuid_session_id,
        )
    }

    fn load_rooms_with(
        &self,
        db: &Arc<Database>,
        store: &RoomStore,
        probe: impl Fn(&Harness) -> Result<bool, String>,
        pick: impl FnMut() -> Result<u16, String>,
        mint: impl FnMut() -> String,
    ) -> Result<LoadOutcome, String> {
        // Held to the end: see the module doc for the lock order. A poisoned
        // guard is recovered, since the flag is always valid.
        let mut done = self.done.lock().unwrap_or_else(PoisonError::into_inner);
        let mut outcome = crate::rooms::load_rooms(db, store)?;
        if !*done {
            *done = true;
            if !outcome.rooms.is_empty()
                && let Err(e) = prepare(store, &mut outcome, probe, pick, mint)
            {
                tracing::error!(error = %e, "boot resume: preparation failed; rooms left unprepared");
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
    mint: impl FnMut() -> String,
) -> Result<(), String> {
    let prepared = prepare_with(outcome.rooms.clone(), probe, pick, mint);
    let mutations = changed_harnesses(&outcome.rooms, &prepared.rooms);
    let dropped = outcome
        .rooms
        .iter()
        .zip(&prepared.rooms)
        .flat_map(|(b, a)| b.harnesses.iter().zip(&a.harnesses))
        .filter(|(hb, ha)| hb.session_id.is_some() && hb.session_id != ha.session_id)
        .count();
    tracing::info!(
        rooms = outcome.rooms.len(),
        cmds_changed = mutations.len(),
        stale_sessions_dropped = dropped,
        "boot resume: prepared the stored rooms"
    );
    if mutations.is_empty() {
        return Ok(());
    }
    let result = store.apply(BOOT_ORIGIN, mutations)?;
    for s in &result.skipped {
        tracing::warn!(index = s.index, reason = %s.reason, "boot resume: mutation skipped");
    }
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
