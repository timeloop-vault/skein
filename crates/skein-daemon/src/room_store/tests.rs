//! `RoomStore` tests (#729). `semantics` covers what a batch does to the
//! in-memory list and the events it emits; `persistence` covers when and
//! how it reaches sqlite. The shared fixture lives here.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::Connection;
use serde_json::{Value, json};
use skein_proto::room_mutations::{
    ROOMS_CHANGED_EVENT, RoomChange, RoomMutation, RoomsApplyOutcome, RoomsChanged,
};
use skein_proto::rooms::{Harness, Room};
use tempfile::TempDir;

use super::RoomStore;
use crate::db::Database;
use crate::db::test_support::fresh_db;
use crate::event_sink::EventSink;

mod failure;
mod load;
mod owned_fields;
mod persistence;
mod semantics;

/// A debounce that cannot fire within a test run.
const NEVER: Duration = Duration::from_secs(600);

/// Records every event; the store only ever emits `ROOMS_CHANGED_EVENT`.
#[derive(Default)]
struct RecordingSink(Mutex<Vec<(String, String)>>);

impl EventSink for RecordingSink {
    fn emit_json(&self, event: &str, payload: String) -> Result<(), String> {
        self.0.lock().unwrap().push((event.to_owned(), payload));
        Ok(())
    }
}

fn room(id: &str) -> Room {
    serde_json::from_value(json!({
        "id": id, "name": format!("room {id}"), "task": "", "status": "idle",
        "badge": 0, "harnesses": [], "activeHarnessId": ""
    }))
    .unwrap()
}

fn harness(id: &str) -> Harness {
    serde_json::from_value(json!({
        "id": id, "kind": "claude", "name": id, "status": "running",
        "model": "", "tokens": "0"
    }))
    .unwrap()
}

fn room_with(id: &str, harness_ids: &[&str]) -> Room {
    let mut r = room(id);
    r.harnesses = harness_ids.iter().map(|h| harness(h)).collect();
    r
}

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

fn json_of<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap()
}

/// A short rendering of a change, for comparing lists: `put:r1`,
/// `removed:r1`, `order:r1,r2`.
fn summarize(c: &RoomChange) -> String {
    match c {
        RoomChange::RoomPut { room } => format!("put:{}", room.id),
        RoomChange::RoomRemoved { room_id } => format!("removed:{room_id}"),
        RoomChange::RoomsOrder { room_ids } => format!("order:{}", room_ids.join(",")),
    }
}

struct Fixture {
    _dir: TempDir,
    path: PathBuf,
    db: Arc<Database>,
    sink: Arc<RecordingSink>,
    store: Arc<RoomStore>,
}

impl Fixture {
    /// A store that has not been filled yet. Its debounce is minutes long,
    /// so only structural batches and `flush` reach sqlite during a test.
    fn unfilled() -> Self {
        Self::unfilled_with_delays(NEVER, NEVER)
    }

    fn unfilled_with_delays(delay: Duration, max_wait: Duration) -> Self {
        let (dir, db) = fresh_db();
        let path = dir.path().join("test.db");
        let db = Arc::new(db);
        let sink = Arc::new(RecordingSink::default());
        let store = RoomStore::with_delays(Arc::clone(&db), sink.clone(), delay, max_wait);
        Self {
            _dir: dir,
            path,
            db,
            sink,
            store,
        }
    }

    /// sqlite holds `rooms` in order, the store was filled from a real
    /// `load_all`, and no events have been recorded yet.
    fn with_rooms(rooms: &[Room]) -> Self {
        Self::with_rooms_and_delays(rooms, NEVER, NEVER)
    }

    fn with_rooms_and_delays(rooms: &[Room], delay: Duration, max_wait: Duration) -> Self {
        let f = Self::unfilled_with_delays(delay, max_wait);
        f.db.write_room_rows(rooms, &[], None).unwrap();
        let out = f.db.load_all().unwrap();
        assert!(out.skipped.is_empty());
        f.store.fill(out.rooms).unwrap();
        f
    }

    fn apply(&self, origin: &str, ms: Vec<RoomMutation>) -> RoomsApplyOutcome {
        self.store.apply(origin, ms).unwrap()
    }

    fn events(&self) -> Vec<RoomsChanged> {
        self.sink
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|(name, payload)| {
                assert_eq!(name, ROOMS_CHANGED_EVENT);
                serde_json::from_str(payload).unwrap()
            })
            .collect()
    }

    /// The change summaries of the single event a batch must have emitted.
    fn only_event(&self) -> (String, Vec<String>) {
        let mut evs = self.events();
        assert_eq!(evs.len(), 1, "expected exactly one event");
        let ev = evs.remove(0);
        (ev.origin, ev.changes.iter().map(summarize).collect())
    }

    /// Reads sqlite through its own connection, not the store's.
    fn raw(&self) -> Connection {
        Connection::open(&self.path).unwrap()
    }

    /// `(id, data)` of every `sessions` row in load order.
    fn disk_rows(&self) -> Vec<(String, String)> {
        let conn = self.raw();
        let mut stmt = conn
            .prepare("SELECT id, data FROM sessions ORDER BY created_at, id")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    fn disk_ids(&self) -> Vec<String> {
        self.disk_rows().into_iter().map(|(id, _)| id).collect()
    }

    fn disk_data(&self, id: &str) -> Option<String> {
        self.disk_rows()
            .into_iter()
            .find(|(i, _)| i == id)
            .map(|(_, d)| d)
    }

    fn disk_room(&self, id: &str) -> Option<Room> {
        self.disk_data(id)
            .map(|d| serde_json::from_str(&d).unwrap())
    }

    fn store_ids(&self) -> Vec<String> {
        self.store.rooms().into_iter().map(|r| r.id).collect()
    }
}
