//! Dirty tracking, the debounce deadline and the background driver (#729).

use std::collections::HashSet;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use skein_proto::rooms::Room;

use crate::db::Database;
use crate::event_sink::SharedEventSink;

/// Quiet period after the last unpersisted change before a debounced write.
pub const ROOMS_PERSIST_DELAY: Duration = Duration::from_millis(500);
/// Longest a change may wait for its write, from the first unpersisted one.
pub const ROOMS_PERSIST_MAX_WAIT: Duration = Duration::from_secs(2);

/// When a debounced write is due: `ROOMS_PERSIST_DELAY` after the latest
/// change, but never later than `ROOMS_PERSIST_MAX_WAIT` after the first
/// unpersisted one.
#[must_use]
pub fn persist_deadline(first_unpersisted: Instant, last_change: Instant) -> Instant {
    deadline_with(
        first_unpersisted,
        last_change,
        ROOMS_PERSIST_DELAY,
        ROOMS_PERSIST_MAX_WAIT,
    )
}

fn deadline_with(
    first_unpersisted: Instant,
    last_change: Instant,
    delay: Duration,
    max_wait: Duration,
) -> Instant {
    (last_change + delay).min(first_unpersisted + max_wait)
}

#[derive(Default)]
pub(super) struct State {
    pub rooms: Vec<Room>,
    pub loaded: bool,
    pub shutdown: bool,
    pub dirty: HashSet<String>,
    pub removed: Vec<String>,
    pub order_dirty: bool,
    /// (first unpersisted change, latest change); `None` when clean.
    pub pending: Option<(Instant, Instant)>,
}

impl State {
    pub fn is_dirty(&self) -> bool {
        !self.dirty.is_empty() || !self.removed.is_empty() || self.order_dirty
    }

    pub fn mark_dirty(&mut self, now: Instant) {
        self.pending = Some(match self.pending {
            Some((first, _)) => (first, now),
            None => (now, now),
        });
    }

    pub fn mark_room(&mut self, id: &str) {
        self.dirty.insert(id.to_owned());
    }

    pub fn mark_removed(&mut self, id: &str) {
        self.dirty.remove(id);
        if !self.removed.iter().any(|r| r == id) {
            self.removed.push(id.to_owned());
        }
    }

    /// A room (re)created under a removed id must not be deleted by the
    /// same write that upserts it.
    pub fn unmark_removed(&mut self, id: &str) {
        self.removed.retain(|r| r != id);
    }

    pub fn clear_dirty(&mut self) {
        self.dirty.clear();
        self.removed.clear();
        self.order_dirty = false;
        self.pending = None;
    }
}

pub(super) struct Shared {
    pub state: Mutex<State>,
    pub wake: Condvar,
    pub db: Arc<Database>,
    pub events: SharedEventSink,
    /// Quiet period and longest wait for a debounced write.
    pub delay: Duration,
    pub max_wait: Duration,
}

impl Shared {
    pub fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Write everything dirty in one `write_room_rows` call. Dirty state is
    /// cleared only on success. Called with the store lock held; the db
    /// connection lock is taken inside.
    pub fn persist(&self, st: &mut State) -> Result<(), String> {
        if !st.is_dirty() {
            st.pending = None;
            return Ok(());
        }
        let upserts: Vec<Room> = st
            .rooms
            .iter()
            .filter(|r| st.dirty.contains(&r.id))
            .cloned()
            .collect();
        let order: Option<Vec<String>> = st
            .order_dirty
            .then(|| st.rooms.iter().map(|r| r.id.clone()).collect());
        self.db
            .write_room_rows(&upserts, &st.removed, order.as_deref())?;
        st.clear_dirty();
        Ok(())
    }

    /// [`Self::persist`] for callers that return the error to a client: on
    /// failure, arm a retry deadline and wake the driver, which may be
    /// parked with nothing pending.
    pub fn persist_or_rearm(&self, st: &mut State) -> Result<(), String> {
        let result = self.persist(st);
        if result.is_err() {
            let now = Instant::now();
            st.pending = Some((now, now));
            self.wake.notify_all();
        }
        result
    }
}

/// The debounce driver. Holds the shared inner only (no reference back to
/// the store), and exits once `shutdown` is set.
pub(super) fn spawn_driver(shared: Arc<Shared>) {
    let spawned = std::thread::Builder::new()
        .name("rooms-persist".into())
        .spawn(move || drive(&shared));
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "room_store: debounce thread failed to start");
    }
}

fn drive(shared: &Shared) {
    let mut st = shared.lock();
    loop {
        if st.shutdown {
            return;
        }
        let Some((first, last)) = st.pending else {
            st = shared.wake.wait(st).unwrap_or_else(PoisonError::into_inner);
            continue;
        };
        let deadline = deadline_with(first, last, shared.delay, shared.max_wait);
        let now = Instant::now();
        if now < deadline {
            st = shared
                .wake
                .wait_timeout(st, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
            continue;
        }
        if let Err(e) = shared.persist(&mut st) {
            tracing::warn!(error = %e, "room_store: debounced persist failed; will retry");
            st.pending = Some((now, now));
        }
    }
}
