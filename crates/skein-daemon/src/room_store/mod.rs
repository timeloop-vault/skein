//! The daemon's in-memory, authoritative copy of the room records (#729).
//!
//! Clients send [`RoomMutation`]s through [`RoomStore::apply`]; the store
//! applies them in order, broadcasts one [`RoomsChanged`] event, and writes
//! per row: structural batches at once, everything else debounced.
//!
//! Lock order: the store lock, then the db connection lock inside
//! `write_room_rows`; never the reverse. Events are emitted under the store
//! lock so they leave in apply order.

mod apply;
mod persist;
mod structural;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Instant;

use skein_proto::room_mutations::{
    ROOMS_CHANGED_EVENT, RoomChange, RoomMutation, RoomsApplyOutcome, RoomsChanged, SkippedMutation,
};
use skein_proto::rooms::Room;

use crate::db::{Database, LoadOutcome};
use crate::event_sink::{self, SharedEventSink};

pub use persist::{ROOMS_PERSIST_DELAY, ROOMS_PERSIST_MAX_WAIT, persist_deadline};
use persist::{Shared, spawn_driver};

pub struct RoomStore {
    shared: Arc<Shared>,
}

impl RoomStore {
    #[must_use]
    pub fn new(db: Arc<Database>, events: SharedEventSink) -> Arc<Self> {
        Self::with_delays(db, events, ROOMS_PERSIST_DELAY, ROOMS_PERSIST_MAX_WAIT)
    }

    /// [`Self::new`] with explicit debounce timing; tests use a very long
    /// delay so the background write cannot fire mid-assertion.
    #[must_use]
    pub(crate) fn with_delays(
        db: Arc<Database>,
        events: SharedEventSink,
        delay: std::time::Duration,
        max_wait: std::time::Duration,
    ) -> Arc<Self> {
        let shared = Arc::new(Shared {
            state: std::sync::Mutex::new(persist::State::default()),
            wake: std::sync::Condvar::new(),
            db,
            events,
            delay,
            max_wait,
        });
        spawn_driver(Arc::clone(&shared));
        Arc::new(Self { shared })
    }

    /// Persist anything dirty, then run `load` and install its rooms, all
    /// under the store lock so no `apply` can commit between the read and
    /// the replace. A persist failure returns before `load` runs, so a
    /// load-once latch inside it is not consumed. Returns the loader's
    /// outcome.
    pub fn load_with(
        &self,
        load: impl FnOnce() -> Result<LoadOutcome, String>,
    ) -> Result<LoadOutcome, String> {
        let mut st = self.shared.lock();
        if st.loaded {
            self.shared.persist_or_rearm(&mut st)?;
        }
        let outcome = load()?;
        st.clear_dirty();
        st.rooms.clone_from(&outcome.rooms);
        st.loaded = true;
        Ok(outcome)
    }

    /// Replace the in-memory list and mark the store loaded. For tests and
    /// first fills only: the caller read `rooms` outside the lock, so use
    /// [`Self::load_with`] wherever `apply` can run concurrently. Anything
    /// still dirty is persisted first; if that fails the list is untouched.
    pub fn fill(&self, rooms: Vec<Room>) -> Result<(), String> {
        let mut st = self.shared.lock();
        if st.loaded {
            self.shared.persist(&mut st)?;
        }
        st.clear_dirty();
        st.rooms = rooms;
        st.loaded = true;
        Ok(())
    }

    /// Apply a batch in order. Refused wholesale until [`Self::fill`] ran.
    pub fn apply(
        &self,
        origin: &str,
        mutations: Vec<RoomMutation>,
    ) -> Result<RoomsApplyOutcome, String> {
        let mut st = self.shared.lock();
        if !st.loaded {
            return Err("rooms store not ready: no rooms load has succeeded yet".into());
        }
        let now = Instant::now();
        let mut skipped = Vec::new();
        let mut touched: Vec<String> = Vec::new();
        let mut removed: Vec<String> = Vec::new();
        let mut order = false;
        let mut structural = false;
        for (index, m) in mutations.into_iter().enumerate() {
            match apply::apply_one(&mut st.rooms, m) {
                Err(reason) => skipped.push(SkippedMutation {
                    index,
                    reason: reason.to_owned(),
                }),
                Ok(a) => {
                    order |= a.order;
                    structural |= a.structural;
                    if let Some(id) = a.touched {
                        st.unmark_removed(&id);
                        st.mark_room(&id);
                        removed.retain(|r| *r != id);
                        if !touched.contains(&id) {
                            touched.push(id);
                        }
                    }
                    if let Some(id) = a.removed {
                        st.mark_removed(&id);
                        touched.retain(|t| *t != id);
                        if !removed.contains(&id) {
                            removed.push(id);
                        }
                    }
                }
            }
        }
        let mut changes: Vec<RoomChange> = removed
            .into_iter()
            .map(|room_id| RoomChange::RoomRemoved { room_id })
            .collect();
        for id in &touched {
            if let Some(room) = st.rooms.iter().find(|r| r.id == *id) {
                changes.push(RoomChange::RoomPut { room: room.clone() });
            }
        }
        if order {
            st.order_dirty = true;
            changes.push(RoomChange::RoomsOrder {
                room_ids: st.rooms.iter().map(|r| r.id.clone()).collect(),
            });
        }
        let outcome = RoomsApplyOutcome { skipped };
        if changes.is_empty() {
            // A retry of mutations whose first attempt failed to persist is
            // answered as skipped; an Ok must still mean the earlier change
            // is on disk, so write anything left dirty now.
            if st.is_dirty() {
                self.shared.persist_or_rearm(&mut st)?;
            }
            return Ok(outcome);
        }
        st.mark_dirty(now);
        let payload = RoomsChanged {
            origin: origin.to_owned(),
            changes,
        };
        if let Err(e) = event_sink::emit(self.shared.events.as_ref(), ROOMS_CHANGED_EVENT, &payload)
        {
            tracing::warn!(error = %e, "room_store: rooms-changed emit failed");
        }
        if structural {
            self.shared.persist_or_rearm(&mut st)?;
        } else {
            self.shared.wake.notify_all();
        }
        Ok(outcome)
    }

    /// Persist everything dirty now.
    pub fn flush(&self) -> Result<(), String> {
        let mut st = self.shared.lock();
        self.shared.persist(&mut st)
    }

    /// A snapshot of the rooms in order.
    #[must_use]
    pub fn rooms(&self) -> Vec<Room> {
        self.shared.lock().rooms.clone()
    }
}

impl Drop for RoomStore {
    fn drop(&mut self) {
        let mut st = self.shared.lock();
        st.shutdown = true;
        if st.loaded
            && let Err(e) = self.shared.persist(&mut st)
        {
            tracing::warn!(error = %e, "room_store: final persist failed");
        }
        drop(st);
        self.shared.wake.notify_all();
    }
}
