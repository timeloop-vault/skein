//! Field-level edits of harness records under the store lock (#755).
//!
//! The daemon owns some harness fields (`session_id`, `opencode_version`);
//! it writes them here, in place, so a client's whole-record put can never
//! race them. Bookkeeping is the one `apply` uses for a changed
//! `PutHarness`.

use std::time::Instant;

use skein_proto::room_mutations::RoomChange;
use skein_proto::rooms::Harness;

use super::RoomStore;
use super::structural::harness_key_eq;

impl RoomStore {
    /// Edit harness records in place, under the store lock (#755). `edit` is
    /// called once per stored harness, in room then harness order, with the
    /// room id and the harness; it changes only the fields it means to.
    /// Returns the number of harnesses that changed.
    ///
    /// `edit` runs under the store lock: it must not call back into the
    /// store and must not block.
    ///
    /// It clones and compares every stored harness under the lock, so it is
    /// for rare batch work (boot); use `edit_harness` for a single record.
    pub fn edit_harnesses(
        &self,
        origin: &str,
        edit: impl FnMut(&str, &mut Harness),
    ) -> Result<usize, String> {
        let (count, snapshot) = self.edit_locked(origin, None, edit)?;
        self.notify_observers(snapshot);
        Ok(count)
    }

    /// The one-harness form: `edit` runs only for that harness. `Ok(false)`
    /// when the harness is not stored or nothing changed.
    ///
    /// `edit` runs under the store lock: it must not call back into the
    /// store and must not block.
    pub fn edit_harness(
        &self,
        origin: &str,
        room_id: &str,
        harness_id: &str,
        edit: impl FnOnce(&mut Harness),
    ) -> Result<bool, String> {
        let mut edit = Some(edit);
        let (count, snapshot) = self.edit_locked(
            origin,
            Some((room_id, harness_id)),
            |_: &str, h: &mut Harness| {
                if let Some(f) = edit.take() {
                    f(h);
                }
            },
        )?;
        self.notify_observers(snapshot);
        Ok(count > 0)
    }

    /// The body of both, returning the observer snapshot (see `apply_locked`).
    fn edit_locked(
        &self,
        origin: &str,
        only: Option<(&str, &str)>,
        mut edit: impl FnMut(&str, &mut Harness),
    ) -> Result<(usize, Option<Vec<skein_proto::rooms::Room>>), String> {
        let mut st = self.shared.lock();
        if !st.loaded {
            return Err("rooms store not ready: no rooms load has succeeded yet".into());
        }
        let now = Instant::now();
        let mut count = 0;
        let mut structural = false;
        let mut touched: Vec<String> = Vec::new();
        for room in &mut st.rooms {
            let mut room_changed = false;
            for h in &mut room.harnesses {
                if only.is_some_and(|(r, hid)| room.id != r || h.id != hid) {
                    continue;
                }
                let before = h.clone();
                edit(&room.id, h);
                debug_assert_eq!(h.id, before.id, "edit must not change the harness id");
                if h.id != before.id {
                    h.id.clone_from(&before.id);
                }
                if serde_json::to_value(&before).ok() != serde_json::to_value(&*h).ok() {
                    count += 1;
                    room_changed = true;
                    structural |= !harness_key_eq(&before, h);
                }
            }
            if room_changed {
                touched.push(room.id.clone());
            }
        }
        if touched.is_empty() {
            // As in `apply`: an Ok must mean earlier changes are on disk.
            if st.is_dirty() {
                self.shared.persist_or_rearm(&mut st)?;
            }
            return Ok((0, None));
        }
        let mut changes = Vec::with_capacity(touched.len());
        for id in &touched {
            st.unmark_removed(id);
            st.mark_room(id);
            if let Some(room) = st.rooms.iter().find(|r| r.id == *id) {
                changes.push(RoomChange::RoomPut { room: room.clone() });
            }
        }
        let snapshot = self.commit_changes(&mut st, origin, changes, structural, now)?;
        Ok((count, snapshot))
    }
}
